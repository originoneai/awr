#![allow(dead_code)]
//! Synthetic API observations, deliberately separate from real HTTPS transport tests.
use awr_server::delivery_adapter::{
    GitHubAdapter, GitHubCheckMapping, GitHubConfig, GitHubError, GitHubResponse, GitHubTransport,
};
use awr_team::{delivery::*, *};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use url::Url;

pub const SOURCE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub const TARGET: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
pub const TREE: &str = "cccccccccccccccccccccccccccccccccccccccc";
pub const BLOB: &str = "dddddddddddddddddddddddddddddddddddddddd";
pub const REWRITTEN: &str = "1111111111111111111111111111111111111111";
pub const BASE_TREE: &str = "2222222222222222222222222222222222222222";
pub const BYTES: &[u8] = b"verified output\n";

pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn run() -> Value {
    json!({"id":1,"head_sha":SOURCE,"name":"CI","app":{"id":9},"status":"completed","conclusion":"success"})
}
pub fn pull() -> Value {
    json!({"number":11,"head":{"sha":SOURCE,"repo":{"id":7}},"base":{"sha":TARGET,"ref":"main","repo":{"id":7}},"state":"open","merged":false})
}
pub fn reference(s: &str) -> Value {
    json!({"ref":"refs/heads/main","object":{"type":"commit","sha":s}})
}

pub fn comparison(base: &str, head: &str, merge_base: &str, status: &str) -> Value {
    json!({"url":format!("https://github.example/api/v3/repos/acme/demo/compare/{base}...{head}"),
        "status":status,"base_commit":{"sha":base},"merge_base_commit":{"sha":merge_base}})
}

#[derive(Default)]
pub struct ApiState {
    pub calls: Vec<String>,
    pub replies: BTreeMap<String, VecDeque<(u16, Vec<u8>)>>,
    pub delay_ms: u64,
}

#[derive(Default)]
pub struct Api(pub Mutex<ApiState>);
impl Api {
    pub fn set(&self, route: &str, value: Value) {
        self.raw(route, 200, serde_json::to_vec(&value).unwrap());
    }
    pub fn raw(&self, route: &str, status: u16, body: Vec<u8>) {
        self.0
            .lock()
            .unwrap()
            .replies
            .insert(route.into(), VecDeque::from([(status, body)]));
    }
    pub fn sequence(&self, route: &str, values: Vec<Value>) {
        self.0.lock().unwrap().replies.insert(
            route.into(),
            values
                .into_iter()
                .map(|v| (200, serde_json::to_vec(&v).unwrap()))
                .collect(),
        );
    }
    pub fn calls(&self) -> usize {
        self.0.lock().unwrap().calls.len()
    }
}
impl GitHubTransport for Api {
    fn get(&self, url: &Url, _: Duration, _: usize) -> Result<GitHubResponse, GitHubError> {
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("github.example"));
        let prefix = "/api/v3/repos/acme/demo";
        assert!(url.path()[..prefix.len()].eq_ignore_ascii_case(prefix));
        let route = url.path()[prefix.len()..].to_owned();
        let route = match url.query() {
            Some(q) => format!("{route}?{q}"),
            None => route,
        };
        let (status, body, delay) = {
            let mut state = self.0.lock().unwrap();
            state.calls.push(route.clone());
            let delay = state.delay_ms;
            if let Some(replies) = state.replies.get_mut(&route) {
                let (status, body) = if replies.len() > 1 {
                    replies.pop_front().unwrap()
                } else {
                    replies.front().unwrap().clone()
                };
                (status, body, delay)
            } else {
                let value = match route.as_str() {
                    "" => {
                        json!({"id":7,"full_name":"acme/demo","archived":false,"permissions":{"push":true}})
                    }
                    "/branches/main" => {
                        json!({"name":"main","protected":false,"commit":{"sha":TARGET}})
                    }
                    "/rules/branches/main" => json!([]),
                    "/git/ref/heads/main" => reference(TARGET),
                    "/pulls/11" => pull(),
                    "/git/commits/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    | "/git/commits/1111111111111111111111111111111111111111" => {
                        json!({"sha":route.rsplit('/').next().unwrap(),"tree":{"sha":TREE}})
                    }
                    "/git/commits/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" => {
                        json!({"sha":TARGET,"tree":{"sha":BASE_TREE}})
                    }
                    "/git/trees/2222222222222222222222222222222222222222" => {
                        json!({"sha":BASE_TREE,"truncated":false,"tree":[]})
                    }
                    "/git/trees/cccccccccccccccccccccccccccccccccccccccc" => {
                        json!({"sha":TREE,"truncated":false,"tree":[{"path":"result.txt","type":"blob","mode":"100644","sha":BLOB,"size":BYTES.len()}]})
                    }
                    "/git/blobs/dddddddddddddddddddddddddddddddddddddddd" => {
                        json!({"sha":BLOB,"size":BYTES.len(),"encoding":"base64","content":STANDARD.encode(BYTES)})
                    }
                    "/commits/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/check-runs?filter=latest&per_page=100&page=1" =>
                    {
                        json!({"total_count":1,"check_runs":[run()]})
                    }
                    "/check-runs/1" => run(),
                    "/compare/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa...bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" => {
                        comparison(SOURCE, TARGET, TARGET, "behind")
                    }
                    "/compare/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb...aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => {
                        comparison(TARGET, SOURCE, TARGET, "ahead")
                    }
                    "/compare/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa...1111111111111111111111111111111111111111" => {
                        comparison(SOURCE, REWRITTEN, TARGET, "diverged")
                    }
                    "/compare/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb...1111111111111111111111111111111111111111" => {
                        comparison(TARGET, REWRITTEN, TARGET, "ahead")
                    }
                    _ => {
                        return Ok(GitHubResponse {
                            status: 404,
                            body: Vec::new(),
                        });
                    }
                };
                (200, serde_json::to_vec(&value).unwrap(), delay)
            }
        };
        if delay > 0 {
            std::thread::sleep(Duration::from_millis(delay));
        }
        Ok(GitHubResponse { status, body })
    }
}

pub struct Fixture {
    pub root: PathBuf,
    pub config: GitHubConfig,
    pub api: Arc<Api>,
}
impl Fixture {
    pub fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-github-fixture-{}", awr_core::Id::new()));
        fs::create_dir(&root).unwrap();
        let config = GitHubConfig {
            version: 1,
            adapter_id: "github-test".into(),
            connector_id: "github-observer".into(),
            tenant_id: "t".into(),
            project_id: "p".into(),
            workstream_id: awr_core::Id::from(1).to_string(),
            work_id: "a".into(),
            resource: "repo:synthetic".into(),
            api_base_url: "https://github.example/api/v3".into(),
            repository_id: 7,
            owner: "acme".into(),
            repository: "demo".into(),
            target_branch: "main".into(),
            pull_number: Some(11),
            checks: vec![GitHubCheckMapping {
                check: "report".into(),
                name: "CI".into(),
                app_id: 9,
            }],
            report_directory: root.clone(),
            request_timeout_ms: 1000,
            inspection_timeout_ms: 10000,
            max_response_bytes: 1024 * 1024,
            max_total_response_bytes: 8 * 1024 * 1024,
            max_blob_bytes: 1024,
            max_total_blob_bytes: 8192,
            max_checks: 300,
            max_requests: 100,
        };
        Self {
            root,
            config,
            api: Arc::new(Api::default()),
        }
    }
    pub fn adapter(&self) -> GitHubAdapter {
        GitHubAdapter::from_transport(self.config.clone(), self.api.clone()).unwrap()
    }
    pub fn candidate(&self) -> DeliveryCandidate {
        let manifest = ArtifactManifest {
            entries: vec![ArtifactEntry {
                artifact_id: "result".into(),
                sha256: hash(BYTES),
                byte_length: BYTES.len().to_string(),
                locator: "git-blob:result.txt".into(),
            }],
        };
        DeliveryCandidate {
            binding: CandidateBinding {
                tenant_id: TenantId::new(&self.config.tenant_id).unwrap(),
                project_id: ProjectId::new(&self.config.project_id).unwrap(),
                scope_id: ScopeId::new("main").unwrap(),
                workstream_id: self.config.workstream_id.clone(),
                work_id: WorkId::new(&self.config.work_id).unwrap(),
                candidate_id: RequestId::new("candidate").unwrap(),
                candidate_version: "1".into(),
                contract_hash: "e".repeat(64),
                manifest_digest: manifest.digest().unwrap(),
                source_revision: Some(RevisionRef {
                    resource: self.config.resource.clone(),
                    format: RevisionFormat::GitSha1,
                    value: SOURCE.into(),
                }),
                required_checks: vec!["report".into()],
                target: DeliveryTarget {
                    resource: self.config.resource.clone(),
                    reference: Some(format!("refs/heads/{}", self.config.target_branch)),
                    precondition: TargetPrecondition::Exact(RevisionRef {
                        resource: self.config.resource.clone(),
                        format: RevisionFormat::GitSha1,
                        value: TARGET.into(),
                    }),
                },
            },
            manifest,
        }
    }
    pub fn files(&self) -> usize {
        fs::read_dir(&self.root).unwrap().count()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
