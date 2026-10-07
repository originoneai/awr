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
        // Exercise the real command boundary with spaces and Unicode on every OS.
        let root = std::env::temp_dir().join(format!("awr local Git 测试-{}", awr_core::Id::new()));
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
        fs::create_dir_all(self.work.join(path).parent().unwrap()).unwrap();
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
    /// An external fixture rewrite; the observation adapter never performs it.
    pub fn publish_tree(&self, tree: &str, parent: Option<&str>) -> String {
        let mut args = vec!["commit-tree", tree, "-m", "Rewritten fixture history"];
        if let Some(parent) = parent {
            args.extend(["-p", parent]);
        }
        let revision = self.git(&args);
        self.git(&[
            "push",
            "--force",
            self.config.repository.to_str().unwrap(),
            &format!("{revision}:refs/heads/main"),
        ]);
        revision
    }

    pub fn rewrite_main(&self, parent: Option<&str>) -> String {
        let tree = self.git(&["rev-parse", &format!("{}^{{tree}}", self.source)]);
        self.publish_tree(&tree, parent)
    }

    pub fn remove_object(&self, revision: &str) {
        let path = self
            .config
            .repository
            .join("objects")
            .join(&revision[..2])
            .join(&revision[2..]);
        assert!(
            path.is_file(),
            "fixture object must be independently removable"
        );
        fs::remove_file(path).unwrap();
    }

    /// Model an archived pre-witness report, without modifying its original bytes.
    pub fn legacy_report(&self, artifact: &ArtifactEntry) -> (String, Vec<u8>) {
        let prefix = if artifact.locator.starts_with("awr-local-git-integration:") {
            "integration-report"
        } else {
            "report"
        };
        let original = self
            .config
            .report_directory
            .join(format!("{prefix}-{}.json", artifact.sha256));
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(original).unwrap()).unwrap();
        value.as_object_mut().unwrap().remove("content_witness");
        if let Some(observation) = value
            .get_mut("observation")
            .and_then(serde_json::Value::as_object_mut)
        {
            observation.remove("content_witness");
        }
        let bytes = serde_json::to_vec(&value).unwrap();
        let hash = sha(&bytes);
        fs::write(
            self.config
                .report_directory
                .join(format!("{prefix}-{hash}.json")),
            &bytes,
        )
        .unwrap();
        let mut changed = 0;
        for entry in fs::read_dir(&self.config.report_directory).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy();
            if name.starts_with("inspection-") || name.starts_with("integration-inspection-") {
                let mut index: serde_json::Value =
                    serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                if index["report_sha256"] == artifact.sha256 {
                    index["report_sha256"] = hash.clone().into();
                    fs::write(path, serde_json::to_vec(&index).unwrap()).unwrap();
                    changed += 1;
                }
            }
        }
        assert_eq!(changed, 1);
        (hash, bytes)
    }

    pub async fn adapter(&self) -> LocalGitAdapter {
        LocalGitAdapter::open(self.config.clone()).await.unwrap()
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

    /// Actual bytes matching the independent review/evidence PG fixture.
    pub fn integration(sha256: bool) -> Self {
        let mut fixture = Self::new(sha256);
        fixture.config.tenant_id = "reader-tenant".into();
        fixture.config.project_id = "reader-project".into();
        fixture.config.connector_id = "git".into();
        fixture.config.resource = "fixture://integration-repository".into();
        fixture.commit("src/api/result.json", b"Reviewed package bytes");
        fixture.source = fixture.git(&["rev-parse", "HEAD"]);
        fixture.git(&[
            "push",
            fixture.config.repository.to_str().unwrap(),
            "HEAD:refs/heads/candidate",
        ]);
        fixture
    }

    pub fn integration_candidate(&self) -> DeliveryCandidate {
        let mut candidate = self.candidate();
        candidate.manifest = ArtifactManifest {
            entries: vec![ArtifactEntry {
                artifact_id: "package".into(),
                sha256: sha(b"Reviewed package bytes"),
                byte_length: "22".into(),
                locator: "git-blob:src/api/result.json".into(),
            }],
        };
        candidate.binding.required_checks = vec!["report".into()];
        rebind(&mut candidate);
        candidate
    }

    /// Independent authoritative source, outside the repository and report spool.
    /// The manifest requirement is activated before any business review.
    pub fn source_contract(&self) -> PathBuf {
        let root = self.root.join("project source");
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(
            root.join("docs/spec.md"),
            "# Delivery contract\nVerify the exact package and preserve compatibility.\n",
        )
        .unwrap();
        fs::write(
            root.join("ledger.yaml"),
            r#"# Preserve operator comments and unrelated work.
description: Source-first local Git delivery
workstreams:
  version: 1
  definitions:
    - id: 00000000000000000000000001
      external_key: alpha
      title: alpha
      state: active
      authority_version: 2
      goal_keys: [alpha]
      acceptance_contracts: [docs/spec.md]
    - id: 00000000000000000000000002
      external_key: private-beta
      title: private-beta
      state: active
      authority_version: 1
      goal_keys: [private-beta]
      acceptance_contracts: []
goals:
  - id: alpha
    title: Alpha
    status: active
  - id: private-beta
    title: Private
    status: active
work_items:
  - id: a
    title: Reviewed API package
    status: planned
    workstream: alpha
    goals: [alpha]
    acceptance: [verified]
    hard_rules: [preserve compatibility]
    paths: [src/api]
    depends_on: []
    completion_policy: caller_managed_execution_and_agent_review
    execution_settlement:
      mode: independent_workspace_v1
      workspace_id: synthetic-workspace-a
    verification_requirements: [local_git.manifest]
  - id: c
    title: Unrelated work
    status: planned
    workstream: alpha
    goals: [alpha]
    acceptance: [verified]
    paths: [other]
    depends_on: []
    completion_policy: ordinary_confirm
    verification_requirements: [report]
  - id: b-private
    title: Private work
    status: planned
    workstream: private-beta
    goals: [private-beta]
    acceptance: [verified]
    paths: [private]
    depends_on: []
    completion_policy: ordinary_confirm
    verification_requirements: [report]
"#,
        )
        .unwrap();
        fs::canonicalize(root).unwrap()
    }
}

pub fn rebind(candidate: &mut DeliveryCandidate) {
    candidate.binding.manifest_digest = candidate.manifest.digest().unwrap();
}
