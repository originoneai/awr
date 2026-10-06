//! Independent synthetic repositories; no production, native client or acceptance credit.
#![allow(dead_code)]
use awr_server::delivery_adapter::{LocalGitAdapter, LocalGitConfig};
use awr_team::delivery::*;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub struct GitFixture {
    pub root: PathBuf,
    pub work: PathBuf,
    pub config: LocalGitConfig,
    pub base: String,
    pub source: String,
    pub format: RevisionFormat,
}

pub fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

impl Drop for GitFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

impl GitFixture {
    pub fn new(sha256: bool) -> Self {
        let root = std::env::temp_dir().join(format!("awr-local-git-{}", awr_core::Id::new()));
        fs::create_dir(&root).unwrap();
        let work = root.join("author");
        let repository = root.join("remote.git");
        let reports = root.join("reports");
        fs::create_dir(&reports).unwrap();
        let name = if cfg!(windows) { "git.exe" } else { "git" };
        let executable = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|p| p.join(name))
            .find(|p| p.is_file())
            .map(|p| fs::canonicalize(p).unwrap())
            .expect("Git is required for adapter conformance");
        let config = LocalGitConfig {
            version: 1,
            adapter_id: "local-fixture".into(),
            connector_id: "local-connector".into(),
            tenant_id: "git-tenant".into(),
            project_id: "git-project".into(),
            workstream_id: awr_core::Id::from(1).to_string(),
            work_id: "a".into(),
            resource: "fixture://local-git".into(),
            repository,
            git_executable: executable,
            target_reference: "refs/heads/main".into(),
            report_directory: reports,
            command_timeout_ms: 3000,
            inspection_timeout_ms: 30000,
            max_blob_bytes: 1048576,
            max_total_blob_bytes: 4194304,
        };
        let mut fixture = Self {
            root,
            work,
            config,
            base: String::new(),
            source: String::new(),
            format: if sha256 {
                RevisionFormat::GitSha256
            } else {
                RevisionFormat::GitSha1
            },
        };
        let algorithm = if sha256 {
            "--object-format=sha256"
        } else {
            "--object-format=sha1"
        };
        fixture.run_at(
            &fixture.root,
            &[
                "init",
                "--initial-branch=main",
                algorithm,
                fixture.work.to_str().unwrap(),
            ],
        );
        fixture.run_at(
            &fixture.root,
            &[
                "init",
                "--bare",
                "--initial-branch=main",
                algorithm,
                fixture.config.repository.to_str().unwrap(),
            ],
        );
        fixture.commit("README.md", b"base\n");
        fixture.base = fixture.git(&["rev-parse", "HEAD"]);
        fixture.git(&[
            "push",
            fixture.config.repository.to_str().unwrap(),
            "HEAD:refs/heads/main",
        ]);
        fixture.commit("deliverable.txt", b"approved content\n");
        fixture.source = fixture.git(&["rev-parse", "HEAD"]);
        fixture.git(&[
            "push",
            fixture.config.repository.to_str().unwrap(),
            "HEAD:refs/heads/candidate",
        ]);
        fixture
    }

    fn run_at(&self, directory: &Path, args: &[&str]) -> String {
        let output = Command::new(&self.config.git_executable)
            .args(["-C", directory.to_str().unwrap()])
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_CONFIG_GLOBAL",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            )
            .env("GIT_AUTHOR_NAME", "Synthetic Author")
            .env("GIT_AUTHOR_EMAIL", "author@example.invalid")
            .env("GIT_COMMITTER_NAME", "Synthetic Author")
            .env("GIT_COMMITTER_EMAIL", "author@example.invalid")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Git fixture failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim_end().into()
    }

    pub fn git(&self, args: &[&str]) -> String {
        self.run_at(&self.work, args)
    }
    pub fn bare(&self, args: &[&str]) -> String {
        self.run_at(&self.config.repository, args)
    }
    pub fn commit(&self, path: &str, bytes: &[u8]) {
        fs::write(self.work.join(path), bytes).unwrap();
        self.git(&["add", "--", path]);
        self.git(&["commit", "-m", "Synthetic delivery revision"]);
    }
    pub fn push_main(&self) {
        self.git(&[
            "push",
            self.config.repository.to_str().unwrap(),
            "HEAD:refs/heads/main",
        ]);
    }
    pub async fn adapter(&self) -> LocalGitAdapter {
        match LocalGitAdapter::open(self.config.clone()).await {
            Ok(adapter) => adapter,
            Err(error) => {
                #[cfg(windows)]
                panic!(
                    "Local Git admission failed: {error:?}; fixed synthetic probe results: {:?}",
                    self.windows_admission_probes()
                );
                #[cfg(not(windows))]
                panic!("Local Git admission failed: {error:?}");
            }
        }
    }

    /// Diagnostic codes only, never command stderr, environment values or arbitrary output.
    #[cfg(windows)]
    fn windows_admission_probes(&self) -> Vec<(bool, bool, Vec<(Option<i32>, bool)>)> {
        let mut results = Vec::new();
        for canonical in [false, true] {
            for platform_environment in [false, true] {
                let repository = if canonical {
                    fs::canonicalize(&self.config.repository).unwrap()
                } else {
                    self.config.repository.clone()
                };
                let mut codes = Vec::new();
                for (args, expected) in [
                    (vec!["rev-parse", "--is-bare-repository"], Some("true")),
                    (vec!["show-ref", "--exists", "refs/heads/main"], None),
                    (vec!["symbolic-ref", "-q", "refs/heads/main"], None),
                    (
                        vec![
                            "config",
                            "--local",
                            "--includes",
                            "--get-regexp",
                            "^(extensions[.]partialclone|remote[.].*[.]promisor)$",
                        ],
                        None,
                    ),
                    (
                        vec!["rev-parse", "--show-object-format"],
                        Some(if self.format == RevisionFormat::GitSha256 {
                            "sha256"
                        } else {
                            "sha1"
                        }),
                    ),
                ] {
                    let mut command = Command::new(&self.config.git_executable);
                    command
                        .args(["--no-replace-objects", "--literal-pathspecs"])
                        .arg(format!("--git-dir={}", repository.display()))
                        .args(args)
                        .current_dir(&repository)
                        .env_clear()
                        .env("PATH", std::env::var_os("PATH").unwrap())
                        .env("GIT_CONFIG_NOSYSTEM", "1")
                        .env("GIT_CONFIG_GLOBAL", "NUL")
                        .env("GIT_NO_REPLACE_OBJECTS", "1")
                        .env("GIT_NO_LAZY_FETCH", "1")
                        .env("GIT_OPTIONAL_LOCKS", "0")
                        .env("GIT_TERMINAL_PROMPT", "0")
                        .env("LC_ALL", "C")
                        .env("LANG", "C");
                    if platform_environment {
                        if let Some(value) = std::env::var_os("SystemRoot") {
                            command.env("SystemRoot", value);
                        }
                    }
                    let output = command.output().unwrap();
                    codes.push((
                        output.status.code(),
                        expected.is_some_and(|value| {
                            String::from_utf8_lossy(&output.stdout).trim_end() == value
                        }),
                    ));
                }
                results.push((canonical, platform_environment, codes));
            }
        }
        results
    }

    pub fn candidate(&self) -> DeliveryCandidate {
        let manifest = ArtifactManifest {
            entries: vec![ArtifactEntry {
                artifact_id: "deliverable".into(),
                sha256: sha(b"approved content\n"),
                byte_length: "17".into(),
                locator: "git-blob:deliverable.txt".into(),
            }],
        };
        serde_json::from_value(json!({"binding":{
            "tenant_id":self.config.tenant_id,"project_id":self.config.project_id,"scope_id":"main","workstream_id":self.config.workstream_id,
            "work_id":self.config.work_id,"candidate_id":"local-candidate","candidate_version":"1","contract_hash":"c".repeat(64),
            "manifest_digest":manifest.digest().unwrap(),"source_revision":{"resource":self.config.resource,"format":self.format,"value":self.source},
            "required_checks":["local_git.manifest","unit-tests"],"target":{"resource":self.config.resource,"reference":self.config.target_reference,
                "precondition":{"kind":"exact","revision":{"resource":self.config.resource,"format":self.format,"value":self.base}}}
        },"manifest":manifest})).unwrap()
    }
}

pub fn rebind(candidate: &mut DeliveryCandidate) {
    candidate.binding.manifest_digest = candidate.manifest.digest().unwrap();
}
