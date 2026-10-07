#![allow(dead_code)]
//! Real isolated Git receive-pack and repository bytes, with synthetic provider
//! identity, permission, policy and CI metadata. No live GitHub acceptance credit.
use super::{git, github};
use awr_server::delivery_adapter::*;
use awr_team::delivery::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{
    io::Write,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use url::Url;

pub struct Api {
    pub repo: Arc<git::GitFixture>,
    pub overrides: github::Api,
    pub calls: Mutex<Vec<String>>,
}

impl GitHubTransport for Api {
    fn get(
        &self,
        url: &Url,
        timeout: Duration,
        limit: usize,
    ) -> Result<GitHubResponse, GitHubError> {
        assert_eq!(url.host_str(), Some("github.example"));
        let route = url.path().strip_prefix("/api/v3/repos/acme/demo").unwrap();
        let full = match url.query() {
            Some(q) => format!("{route}?{q}"),
            None => route.into(),
        };
        self.calls.lock().unwrap().push(full.clone());
        if self.overrides.0.lock().unwrap().replies.contains_key(&full) {
            return self.overrides.get(url, timeout, limit);
        }
        let target = self.repo.bare(&["rev-parse", "refs/heads/main"]);
        let source = &self.repo.source;
        let value = if route.is_empty() {
            json!({"id":7,"full_name":"acme/demo","archived":false,"permissions":{"push":true}})
        } else if route == "/git/ref/heads/main" {
            github::reference(&target)
        } else if route == "/branches/main" {
            json!({"name":"main","protected":false,"commit":{"sha":target}})
        } else if route == "/rules/branches/main" {
            json!([])
        } else if route == "/pulls/11" {
            json!({"number":11,"head":{"sha":source,"repo":{"id":7}},"base":{"sha":target,"ref":"main","repo":{"id":7}},"state":"open","merged":false})
        } else if let Some(id) = route.strip_prefix("/git/commits/") {
            json!({"sha":id,"tree":{"sha":self.repo.bare(&["rev-parse",&format!("{id}^{{tree}}")])}})
        } else if let Some(id) = route.strip_prefix("/git/trees/") {
            let entries: Vec<Value> = self
                .repo
                .bare(&["ls-tree", "--long", id])
                .lines()
                .map(|line| {
                    let (meta, path) = line.split_once('\t').unwrap();
                    let parts: Vec<_> = meta.split_whitespace().collect();
                    let mut value =
                        json!({"mode":parts[0],"type":parts[1],"sha":parts[2],"path":path});
                    if let Ok(size) = parts[3].parse::<u64>() {
                        value["size"] = size.into();
                    }
                    value
                })
                .collect();
            json!({"sha":id,"truncated":false,"tree":entries})
        } else if let Some(id) = route.strip_prefix("/git/blobs/") {
            let bytes = self.repo.bare(&["cat-file", "blob", id]);
            json!({"sha":id,"size":bytes.len(),"encoding":"base64","content":STANDARD.encode(bytes)})
        } else if route.starts_with("/commits/") && route.ends_with("/check-runs") {
            json!({"total_count":1,"check_runs":[{"id":1,"head_sha":source,"name":"CI","app":{"id":9},"status":"completed","conclusion":"success"}]})
        } else if route == "/check-runs/1" {
            json!({"id":1,"head_sha":source,"name":"CI","app":{"id":9},"status":"completed","conclusion":"success"})
        } else if let Some(pair) = route.strip_prefix("/compare/") {
            let (a, b) = pair.split_once("...").unwrap();
            let merge = Command::new(&self.repo.config.git_executable)
                .args([
                    "-C",
                    self.repo.config.repository.to_str().unwrap(),
                    "merge-base",
                    a,
                    b,
                ])
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env(
                    "GIT_CONFIG_GLOBAL",
                    if cfg!(windows) { "NUL" } else { "/dev/null" },
                )
                .output()
                .unwrap();
            if !merge.status.success() {
                return Ok(GitHubResponse {
                    status: 404,
                    body: vec![],
                });
            }
            let ancestor = String::from_utf8(merge.stdout)
                .unwrap()
                .trim_end()
                .to_owned();
            let status = if a == b {
                "identical"
            } else if ancestor == a {
                "ahead"
            } else if ancestor == b {
                "behind"
            } else {
                "diverged"
            };
            github::comparison(a, b, &ancestor, status)
        } else {
            return Ok(GitHubResponse {
                status: 404,
                body: Vec::new(),
            });
        };
        Ok(GitHubResponse {
            status: 200,
            body: serde_json::to_vec(&value).unwrap(),
        })
    }
}

pub struct Receive {
    pub repo: Arc<git::GitFixture>,
    pub posts: AtomicUsize,
    pub adverts: AtomicUsize,
    pub lose_reply: AtomicBool,
    pub malformed_reply: AtomicBool,
    pub drift_on_post: Mutex<Option<String>>,
    pub post_delay_ms: AtomicUsize,
}

impl GitHubReceiveTransport for Receive {
    fn request(
        &self,
        method: GitHubReceiveMethod,
        url: &Url,
        body: &[u8],
        _: Duration,
        limit: usize,
    ) -> Result<GitHubReceiveResponse, GitHubError> {
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("github.example"));
        let advertise = method == GitHubReceiveMethod::Advertise;
        assert_eq!(
            url.path(),
            if advertise {
                "/acme/demo.git/info/refs"
            } else {
                "/acme/demo.git/git-receive-pack"
            }
        );
        if advertise {
            assert!(body.is_empty());
            self.adverts.fetch_add(1, Ordering::SeqCst);
        } else {
            self.posts.fetch_add(1, Ordering::SeqCst);
            if let Some(revision) = self.drift_on_post.lock().unwrap().take() {
                self.repo
                    .bare(&["update-ref", "refs/heads/main", &revision]);
            }
        }
        let mut command = Command::new(&self.repo.config.git_executable);
        command.args(["receive-pack", "--stateless-rpc"]);
        if advertise {
            command.arg("--advertise-refs");
        }
        command
            .arg(&self.repo.config.repository)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_CONFIG_GLOBAL",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        child.stdin.take().unwrap().write_all(body).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "isolated Git receive-pack failed");
        let mut bytes = Vec::new();
        if advertise {
            bytes.extend(b"001f# service=git-receive-pack\n0000");
        }
        bytes.extend(output.stdout);
        if !advertise {
            std::thread::sleep(Duration::from_millis(
                self.post_delay_ms.load(Ordering::SeqCst) as u64,
            ));
            if self.lose_reply.load(Ordering::SeqCst) {
                return Err(GitHubError::TimedOut);
            }
            if self.malformed_reply.load(Ordering::SeqCst) {
                bytes = b"invalid acknowledgement".to_vec();
            }
        }
        assert!(bytes.len() <= limit);
        Ok(GitHubReceiveResponse {
            status: 200,
            content_type: Some(
                if advertise {
                    "application/x-git-receive-pack-advertisement"
                } else {
                    "application/x-git-receive-pack-result"
                }
                .into(),
            ),
            body: bytes,
        })
    }
}

pub struct Fixture {
    pub repo: Arc<git::GitFixture>,
    pub api: Arc<Api>,
    pub receive: Arc<Receive>,
    pub config: GitHubIntegrationConfig,
}
impl Fixture {
    pub fn new() -> Self {
        let repo = Arc::new(git::GitFixture::integration(false));
        let defaults = github::Fixture::new();
        let mut config = defaults.config.clone();
        config.tenant_id = repo.config.tenant_id.clone();
        config.project_id = repo.config.project_id.clone();
        config.connector_id = "git".into();
        config.resource = repo.config.resource.clone();
        config.report_directory = repo.config.report_directory.clone();
        let api = Arc::new(Api {
            repo: repo.clone(),
            overrides: github::Api::default(),
            calls: Mutex::new(Vec::new()),
        });
        let receive = Arc::new(Receive {
            repo: repo.clone(),
            posts: AtomicUsize::new(0),
            adverts: AtomicUsize::new(0),
            lose_reply: AtomicBool::new(false),
            malformed_reply: AtomicBool::new(false),
            drift_on_post: Mutex::new(None),
            post_delay_ms: AtomicUsize::new(0),
        });
        Self {
            repo,
            api,
            receive,
            config: GitHubIntegrationConfig {
                enabled: true,
                repository: config,
            },
        }
    }
    pub fn candidate(&self) -> DeliveryCandidate {
        self.repo.integration_candidate()
    }
    pub fn integrator(&self) -> GitHubIntegrator {
        GitHubIntegrator::from_transports(
            self.config.clone(),
            self.api.clone(),
            self.receive.clone(),
        )
        .unwrap()
    }

    /// Construct an immutable pre-witness archive with its original valid index.
    pub fn legacy_integration_report(&self, artifact: &ArtifactEntry) -> (String, Vec<u8>) {
        let integrator = self.integrator();
        let mut report: Value =
            serde_json::from_slice(&integrator.report_bytes(&artifact.sha256).unwrap()).unwrap();
        report.as_object_mut().unwrap().remove("content_witness");
        report["observation"]
            .as_object_mut()
            .unwrap()
            .remove("content_witness");
        let report: GitHubIntegrationReport = serde_json::from_value(report).unwrap();
        let bytes = serde_json::to_vec(&report).unwrap();
        let hash = github::hash(&bytes);
        let directory = &self.config.repository.report_directory;
        std::fs::write(
            directory.join(format!("github-integration-report-{hash}.json")),
            &bytes,
        )
        .unwrap();
        let mut changed = 0;
        for entry in std::fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            if !entry
                .file_name()
                .to_string_lossy()
                .starts_with("github-integration-inspection-")
            {
                continue;
            }
            let mut pointer: Value =
                serde_json::from_slice(&std::fs::read(entry.path()).unwrap()).unwrap();
            if pointer["report_sha256"] == artifact.sha256 {
                pointer["report_sha256"] = json!(hash);
                std::fs::write(entry.path(), serde_json::to_vec(&pointer).unwrap()).unwrap();
                changed += 1;
            }
        }
        assert_eq!(changed, 1);
        (hash, bytes)
    }
}
