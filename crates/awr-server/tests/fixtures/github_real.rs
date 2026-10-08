#![allow(dead_code)]
//! Real GitHub responses for the observation adapter.
//!
//! `fixtures/github.rs` answers with minimal synthetic JSON. Here the adapter reads what
//! api.github.com actually returned (recorded unauthenticated, see the `about` field of the
//! recording): full-size payloads with every extra field, a base64 blob that ends in a newline,
//! compare URLs as GitHub spells them. The same candidates run against the live API in
//! `github_live.rs`.
use awr_server::delivery_adapter::{
    GitHubAdapter, GitHubConfig, GitHubError, GitHubResponse, GitHubTransport,
    local_git::ArtifactState,
};
use awr_team::{delivery::*, *};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use url::Url;

pub const OWNER: &str = "octocat";
pub const REPOSITORY: &str = "Hello-World";
pub const RESOURCE: &str = "repo:octocat-hello-world";
/// What `master` pointed at when the responses were recorded, and its parent. Both commits
/// carry the same tree, so the single file `README` is identical in both.
pub const TIP: &str = "7fd1a60b01f91b314f59955a4e4d4e80d8edf11d";
pub const PARENT: &str = "762941318ee16e59dabbacb1b4049eec22f0d303";
pub const README: &[u8] = b"Hello World!\n";
const RECORDING: &str = include_str!("github-real/octocat-hello-world.json");

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn repository_id() -> u64 {
    let recording: Value = serde_json::from_str(RECORDING).unwrap();
    recording["repository_id"].as_u64().unwrap()
}

/// Replays the recording. A route that was not recorded is a test failure, so a change in
/// the requests the adapter makes cannot go unnoticed.
pub struct Recorded {
    routes: BTreeMap<String, (u16, Vec<u8>)>,
    pub calls: Mutex<Vec<String>>,
}

impl Recorded {
    pub fn load() -> Self {
        let recording: Value = serde_json::from_str(RECORDING).unwrap();
        let routes = recording["routes"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(route, reply)| {
                (
                    route.clone(),
                    (
                        reply["status"].as_u64().unwrap() as u16,
                        serde_json::to_vec(&reply["body"]).unwrap(),
                    ),
                )
            })
            .collect();
        Self {
            routes,
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl GitHubTransport for Recorded {
    fn get(&self, url: &Url, _: Duration, _: usize) -> Result<GitHubResponse, GitHubError> {
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("api.github.com"));
        let prefix = format!("/repos/{OWNER}/{REPOSITORY}");
        let path = url.path();
        assert!(path.starts_with(&prefix), "unexpected path {path}");
        let mut route = path[prefix.len()..].to_owned();
        if let Some(query) = url.query() {
            route = format!("{route}?{query}");
        }
        self.calls.lock().unwrap().push(route.clone());
        match self.routes.get(&route) {
            Some((status, body)) => Ok(GitHubResponse {
                status: *status,
                body: body.clone(),
            }),
            None => panic!("the adapter asked for a route that was not recorded: {route}"),
        }
    }
}

pub struct Scratch(pub PathBuf);
impl Scratch {
    pub fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-github-real-{}", awr_core::Id::new()));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn config(report_directory: PathBuf, repository_id: u64) -> GitHubConfig {
    GitHubConfig {
        version: 1,
        adapter_id: "github-real".into(),
        connector_id: "github-observer".into(),
        tenant_id: "t".into(),
        project_id: "p".into(),
        workstream_id: awr_core::Id::from(1).to_string(),
        work_id: "a".into(),
        resource: RESOURCE.into(),
        api_base_url: "https://api.github.com".into(),
        repository_id,
        owner: OWNER.into(),
        repository: REPOSITORY.into(),
        target_branch: "master".into(),
        pull_number: None,
        checks: vec![],
        report_directory,
        request_timeout_ms: 10_000,
        inspection_timeout_ms: 60_000,
        max_response_bytes: 1024 * 1024,
        max_total_response_bytes: 8 * 1024 * 1024,
        max_blob_bytes: 1024,
        max_total_blob_bytes: 8192,
        max_checks: 300,
        max_requests: 100,
    }
}

pub fn recorded_adapter(scratch: &Scratch) -> (GitHubAdapter, Arc<Recorded>) {
    let api = Arc::new(Recorded::load());
    let adapter =
        GitHubAdapter::from_transport(config(scratch.0.clone(), repository_id()), api.clone())
            .unwrap();
    (adapter, api)
}

/// A candidate that changes nothing: the README it names is the one in the commit `source`.
pub fn candidate(source: &str, base: &str) -> DeliveryCandidate {
    let manifest = ArtifactManifest {
        entries: vec![ArtifactEntry {
            artifact_id: "readme".into(),
            sha256: sha256(README),
            byte_length: README.len().to_string(),
            locator: "git-blob:README".into(),
        }],
    };
    let revision = |value: &str| RevisionRef {
        resource: RESOURCE.into(),
        format: RevisionFormat::GitSha1,
        value: value.into(),
    };
    DeliveryCandidate {
        binding: CandidateBinding {
            tenant_id: TenantId::new("t").unwrap(),
            project_id: ProjectId::new("p").unwrap(),
            scope_id: ScopeId::new("main").unwrap(),
            workstream_id: awr_core::Id::from(1).to_string(),
            work_id: WorkId::new("a").unwrap(),
            candidate_id: RequestId::new("candidate").unwrap(),
            candidate_version: "1".into(),
            contract_hash: "e".repeat(64),
            manifest_digest: manifest.digest().unwrap(),
            source_revision: Some(revision(source)),
            required_checks: vec![],
            target: DeliveryTarget {
                resource: RESOURCE.into(),
                reference: Some("refs/heads/master".into()),
                precondition: TargetPrecondition::Exact(revision(base)),
            },
        },
        manifest,
    }
}

/// What both scenarios must report, whether the answers are replayed or live. `source_is_tip`
/// selects the scenario: the candidate's source is the tip itself (exact revision) or its
/// parent (an ancestor with an identical tree).
pub fn assert_applied(report: &awr_server::delivery_adapter::GitHubReport, source_is_tip: bool) {
    let source = if source_is_tip { TIP } else { PARENT };
    assert_eq!(report.repository_id, repository_id());
    assert_eq!(report.source_revision.value, source);
    assert_eq!(report.manifest_outcome, VerificationOutcome::Passed);
    assert_eq!(report.source_artifacts.len(), 1);
    assert_eq!(report.source_artifacts[0].state, ArtifactState::Passed);
    assert_eq!(
        report.source_artifacts[0].observed_sha256.as_deref(),
        Some(sha256(README).as_str())
    );
    assert_eq!(
        report.source_artifacts[0].observed_byte_length.as_deref(),
        Some("13")
    );
    assert_eq!(
        report.target_revision.as_ref().map(|r| r.value.as_str()),
        Some(TIP)
    );
    assert_eq!(report.graph_contains_source, Some(true));
    assert!(report.target_stable && report.pull_stable);
    assert!(
        !report.target_precondition_matches,
        "the target is already past the base"
    );
    assert_eq!(report.integration_outcome, IntegrationOutcome::Applied);
    match (&report.content_witness, source_is_tip) {
        (Some(IntegrationContentWitness::ExactRevision), true) => {}
        (Some(IntegrationContentWitness::MatchingCompleteSnapshots { .. }), false) => {}
        (other, _) => panic!("unexpected content witness: {other:?}"),
    }
}
