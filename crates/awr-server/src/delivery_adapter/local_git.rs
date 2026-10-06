use super::{
    LocalGitError, REPORT_LIMIT, digest, git_process::GitProcess, publish, read_file, text,
};
use awr_team::delivery::*;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const MANIFEST_CHECK: &str = "local_git.manifest";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalGitConfig {
    pub version: u32,
    pub adapter_id: String,
    pub connector_id: String,
    pub tenant_id: String,
    pub project_id: String,
    pub workstream_id: String,
    pub work_id: String,
    pub resource: String,
    /// Existing operator-owned bare repository, never an RPC-supplied path or remote URL.
    pub repository: PathBuf,
    pub git_executable: PathBuf,
    pub target_reference: String,
    /// Existing private directory outside the repository for immutable reports.
    pub report_directory: PathBuf,
    pub command_timeout_ms: u64,
    pub inspection_timeout_ms: u64,
    pub max_blob_bytes: usize,
    pub max_total_blob_bytes: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactState {
    Passed,
    Mismatch,
    Unavailable,
    Unsupported,
    TooLarge,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactObservation {
    pub artifact_id: String,
    pub state: ArtifactState,
    pub observed_sha256: Option<String>,
    pub observed_byte_length: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalGitReport {
    pub version: u32,
    pub adapter_id: String,
    pub inspection_id: String,
    pub config_digest: String,
    pub binding_digest: String,
    pub observed_at_unix_ms: u64,
    pub source_revision: Option<RevisionRef>,
    pub source_artifacts: Vec<ArtifactObservation>,
    pub manifest_outcome: VerificationOutcome,
    pub unsupported_required_checks: Vec<String>,
    pub target_revision: Option<RevisionRef>,
    pub target_artifacts: Vec<ArtifactObservation>,
    pub graph_contains_source: Option<bool>,
    pub target_stable: bool,
    pub target_precondition_matches: bool,
    pub integration_outcome: IntegrationOutcome,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalGitSnapshot {
    pub report: LocalGitReport,
    pub report_artifact: ArtifactEntry,
    pub records: Vec<DeliveryEnvelope>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InspectionIndex {
    config_digest: String,
    binding_digest: String,
    report_sha256: String,
}

pub struct LocalGitAdapter {
    pub(super) config: LocalGitConfig,
    pub(super) git: GitProcess,
    pub(super) format: RevisionFormat,
    pub(super) config_digest: String,
}

fn valid_ref(value: &str) -> bool {
    value.starts_with("refs/heads/")
        && text(value, 1024)
        && !value.contains([' ', '~', '^', ':', '?', '*', '[', '\\'])
        && !value.contains("..")
        && !value.contains("@{")
        && value.split('/').all(|p| {
            !p.is_empty() && !p.starts_with('.') && !p.ends_with('.') && !p.ends_with(".lock")
        })
}

fn blob_path(locator: &str) -> Option<&str> {
    let path = locator.strip_prefix("git-blob:")?;
    (text(path, 1024)
        && !path.contains(['\\', ':'])
        && path
            .split('/')
            .all(|p| !p.is_empty() && p != "." && p != ".."))
    .then_some(path)
}

impl LocalGitAdapter {
    pub async fn open(mut config: LocalGitConfig) -> Result<Self, LocalGitError> {
        if config.version != 1
            || !valid_ref(&config.target_reference)
            || [
                &config.adapter_id,
                &config.connector_id,
                &config.tenant_id,
                &config.project_id,
                &config.workstream_id,
                &config.work_id,
            ]
            .iter()
            .any(|s| !text(s, 128))
            || config.workstream_id.parse::<awr_core::Id>().is_err()
            || !text(&config.resource, 1024)
            || !(100..=10000).contains(&config.command_timeout_ms)
            || !(config.command_timeout_ms..=60000).contains(&config.inspection_timeout_ms)
            || !(1..=8 * 1024 * 1024).contains(&config.max_blob_bytes)
            || !(config.max_blob_bytes..=32 * 1024 * 1024).contains(&config.max_total_blob_bytes)
            || [
                &config.repository,
                &config.git_executable,
                &config.report_directory,
            ]
            .iter()
            .any(|p| !p.is_absolute())
        {
            return Err(LocalGitError::InvalidConfiguration);
        }
        config.repository = fs::canonicalize(&config.repository)
            .map_err(|_| LocalGitError::InvalidConfiguration)?;
        config.git_executable = fs::canonicalize(&config.git_executable)
            .map_err(|_| LocalGitError::InvalidConfiguration)?;
        config.report_directory = fs::canonicalize(&config.report_directory)
            .map_err(|_| LocalGitError::InvalidConfiguration)?;
        if !config.repository.is_dir()
            || !config.report_directory.is_dir()
            || !config.git_executable.is_file()
            || config.report_directory.starts_with(&config.repository)
        {
            return Err(LocalGitError::InvalidConfiguration);
        }
        let git = GitProcess {
            executable: config.git_executable.clone(),
            repository: config.repository.clone(),
            timeout_ms: config.command_timeout_ms,
        };
        let format =
            tokio::time::timeout(Duration::from_millis(config.inspection_timeout_ms), async {
                if git
                    .text(&["rev-parse", "--is-bare-repository"])
                    .await?
                    .as_deref()
                    != Some("true")
                    || !matches!(
                        git.run(&["show-ref", "--exists", &config.target_reference], 4096,)
                            .await?
                            .code,
                        0 | 2
                    )
                    || git
                        .text(&["symbolic-ref", "-q", &config.target_reference])
                        .await?
                        .is_some()
                {
                    return Err(LocalGitError::InvalidConfiguration);
                }
                // Even old Git versions must never turn a missing object into a network fetch.
                let partial = git
                    .run(
                        &[
                            "config",
                            "--local",
                            "--includes",
                            "--get-regexp",
                            "^(extensions[.]partialclone|remote[.].*[.]promisor)$",
                        ],
                        4096,
                    )
                    .await?;
                if partial.code != 1 {
                    return Err(LocalGitError::InvalidConfiguration);
                }
                match git
                    .text(&["rev-parse", "--show-object-format"])
                    .await?
                    .as_deref()
                {
                    Some("sha1") => Ok(RevisionFormat::GitSha1),
                    Some("sha256") => Ok(RevisionFormat::GitSha256),
                    _ => Err(LocalGitError::InvalidConfiguration),
                }
            })
            .await
            .map_err(|_| LocalGitError::TimedOut)??;
        let config_digest =
            digest(&serde_json::to_vec(&config).map_err(|_| LocalGitError::InvalidConfiguration)?);
        Ok(Self {
            config,
            git,
            format,
            config_digest,
        })
    }

    pub fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            adapter_id: self.config.adapter_id.clone(),
            inspection: true,
            change_requests: false,
            verification: true,
            integration_requests: false,
            integration_observations: true,
            notifications: false,
            polling: true,
        }
    }

    pub(super) fn validate_candidate(
        &self,
        candidate: &DeliveryCandidate,
    ) -> Result<(), LocalGitError> {
        DeliveryRecord::Candidate(candidate.clone())
            .validate()
            .map_err(|_| LocalGitError::BindingMismatch)?;
        let binding = &candidate.binding;
        if binding.tenant_id.as_str() != self.config.tenant_id
            || binding.project_id.as_str() != self.config.project_id
            || binding.scope_id.as_str() != "main"
            || binding.work_id.as_str() != self.config.work_id
            || binding.workstream_id != self.config.workstream_id
            || binding.target.resource != self.config.resource
            || binding.target.reference.as_deref() != Some(&self.config.target_reference)
            || binding
                .source_revision
                .as_ref()
                .is_some_and(|s| s.resource != self.config.resource)
        {
            return Err(LocalGitError::BindingMismatch);
        }
        Ok(())
    }

    /// Repeating the same inspection returns its immutable original observation.
    /// A new inspection ID is required to observe a changed repository.
    pub async fn inspect(
        &self,
        candidate: &DeliveryCandidate,
        inspection_id: &str,
    ) -> Result<LocalGitSnapshot, LocalGitError> {
        self.validate_candidate(candidate)?;
        if !text(inspection_id, 128) {
            return Err(LocalGitError::BindingMismatch);
        }
        let binding_digest = candidate
            .binding
            .digest()
            .map_err(|_| LocalGitError::BindingMismatch)?;
        let key = digest(&serde_json::to_vec(&(&self.config.adapter_id, inspection_id)).unwrap());
        let index_path = self
            .config
            .report_directory
            .join(format!("inspection-{key}.json"));
        if let Some(snapshot) = self.cached(&index_path, candidate, inspection_id)? {
            return Ok(snapshot);
        }
        let report = tokio::time::timeout(
            Duration::from_millis(self.config.inspection_timeout_ms),
            self.observe(candidate, inspection_id, binding_digest.clone()),
        )
        .await
        .map_err(|_| LocalGitError::TimedOut)??;
        let bytes = serde_json::to_vec(&report).map_err(|_| LocalGitError::ReportUnavailable)?;
        let report_sha256 = digest(&bytes);
        publish(
            &self
                .config
                .report_directory
                .join(format!("report-{report_sha256}.json")),
            &bytes,
        )?;
        let index = InspectionIndex {
            config_digest: self.config_digest.clone(),
            binding_digest,
            report_sha256,
        };
        let bytes = serde_json::to_vec(&index).map_err(|_| LocalGitError::ReportUnavailable)?;
        match publish(&index_path, &bytes) {
            Ok(()) | Err(LocalGitError::ReportConflict) => self
                .cached(&index_path, candidate, inspection_id)?
                .ok_or(LocalGitError::ReportUnavailable),
            Err(error) => Err(error),
        }
    }

    pub fn report_bytes(&self, sha256: &str) -> Result<Vec<u8>, LocalGitError> {
        if sha256.len() != 64
            || !sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(LocalGitError::ReportUnavailable);
        }
        let bytes = read_file(
            &self
                .config
                .report_directory
                .join(format!("report-{sha256}.json")),
        )?
        .ok_or(LocalGitError::ReportUnavailable)?;
        if digest(&bytes) != sha256 {
            return Err(LocalGitError::ReportConflict);
        }
        Ok(bytes)
    }

    fn cached(
        &self,
        index: &std::path::Path,
        candidate: &DeliveryCandidate,
        inspection_id: &str,
    ) -> Result<Option<LocalGitSnapshot>, LocalGitError> {
        let Some(bytes) = read_file(index)? else {
            return Ok(None);
        };
        let index: InspectionIndex =
            serde_json::from_slice(&bytes).map_err(|_| LocalGitError::ReportUnavailable)?;
        if index.config_digest != self.config_digest
            || index.binding_digest
                != candidate
                    .binding
                    .digest()
                    .map_err(|_| LocalGitError::BindingMismatch)?
        {
            return Err(LocalGitError::ReportConflict);
        }
        let bytes = self.report_bytes(&index.report_sha256)?;
        let report: LocalGitReport =
            serde_json::from_slice(&bytes).map_err(|_| LocalGitError::ReportUnavailable)?;
        if report.version != 1
            || report.config_digest != self.config_digest
            || report.binding_digest != index.binding_digest
            || report.inspection_id != inspection_id
            || report.adapter_id != self.config.adapter_id
        {
            return Err(LocalGitError::ReportConflict);
        }
        let report_artifact = ArtifactEntry {
            artifact_id: format!("local-git-report-{}", index.report_sha256),
            sha256: index.report_sha256.clone(),
            byte_length: bytes.len().to_string(),
            locator: format!(
                "awr-local-git-report:{}:{}",
                self.config.adapter_id, index.report_sha256
            ),
        };
        let provenance = FactProvenance {
            source: FactSource::AdapterObservation,
            reference: report_artifact.locator.clone(),
            observed_at_unix_ms: Some(report.observed_at_unix_ms),
            recorded_at_unix_ms: report.observed_at_unix_ms,
        };
        let records = vec![
            DeliveryEnvelope {
                protocol: DELIVERY_PROTOCOL.into(),
                protocol_version: DELIVERY_PROTOCOL_VERSION,
                record: DeliveryRecord::Verification(VerificationRun {
                    binding: candidate.binding.clone(),
                    run_id: format!("local-git-{}", index.report_sha256),
                    check: MANIFEST_CHECK.into(),
                    outcome: report.manifest_outcome.clone(),
                    result_artifact: Some(report_artifact.clone()),
                    provenance: provenance.clone(),
                }),
            },
            DeliveryEnvelope {
                protocol: DELIVERY_PROTOCOL.into(),
                protocol_version: DELIVERY_PROTOCOL_VERSION,
                record: DeliveryRecord::IntegrationObservation(IntegrationObservation {
                    binding: candidate.binding.clone(),
                    request_id: None,
                    external_reference: format!("local-git-target:{}", self.config.adapter_id),
                    outcome: report.integration_outcome.clone(),
                    result_revision: report.target_revision.clone(),
                    contains_manifest_digest: (report.integration_outcome
                        == IntegrationOutcome::Applied)
                        .then(|| candidate.binding.manifest_digest.clone()),
                    provenance,
                }),
            },
        ];
        for record in &records {
            record
                .validate()
                .map_err(|_| LocalGitError::ReportConflict)?;
        }
        Ok(Some(LocalGitSnapshot {
            report,
            report_artifact,
            records,
        }))
    }

    async fn target(&self) -> Result<Option<RevisionRef>, LocalGitError> {
        if self
            .git
            .text(&["symbolic-ref", "-q", &self.config.target_reference])
            .await?
            .is_some()
        {
            return Err(LocalGitError::RepositoryUnavailable);
        }
        // Git 2.46+ distinguishes present (0), missing (2) and query errors (1).
        // --verify --quiet conflates a broken loose ref with a missing one.
        match self
            .git
            .run(
                &["show-ref", "--exists", &self.config.target_reference],
                4096,
            )
            .await?
            .code
        {
            0 => {}
            2 => return Ok(None),
            _ => return Err(LocalGitError::RepositoryUnavailable),
        }
        let Some(value) = self
            .git
            .text(&[
                "show-ref",
                "--verify",
                "--hash",
                &self.config.target_reference,
            ])
            .await?
        else {
            return Err(LocalGitError::RepositoryUnavailable);
        };
        let revision = RevisionRef {
            resource: self.config.resource.clone(),
            format: self.format.clone(),
            value,
        };
        revision
            .validate()
            .map_err(|_| LocalGitError::RepositoryUnavailable)?;
        if self
            .git
            .text(&["cat-file", "-t", &revision.value])
            .await?
            .as_deref()
            != Some("commit")
        {
            return Err(LocalGitError::RepositoryUnavailable);
        }
        Ok(Some(revision))
    }

    async fn artifacts(
        &self,
        manifest: &ArtifactManifest,
        revision: &str,
        remaining: &mut usize,
    ) -> Result<Vec<ArtifactObservation>, LocalGitError> {
        let mut observed = Vec::new();
        for entry in &manifest.entries {
            let mut result = ArtifactObservation {
                artifact_id: entry.artifact_id.clone(),
                state: ArtifactState::Unsupported,
                observed_sha256: None,
                observed_byte_length: None,
            };
            if let Some(path) = blob_path(&entry.locator) {
                let listing = self
                    .git
                    .run(&["ls-tree", "-z", revision, "--", path], 4096)
                    .await?;
                result.state = ArtifactState::Unavailable;
                if listing.code == 0 && !listing.bytes.is_empty() {
                    let listing = std::str::from_utf8(&listing.bytes)
                        .map_err(|_| LocalGitError::RepositoryUnavailable)?;
                    let (header, listed_path) = listing
                        .strip_suffix('\0')
                        .and_then(|s| s.split_once('\t'))
                        .ok_or(LocalGitError::RepositoryUnavailable)?;
                    let parts: Vec<_> = header.split(' ').collect();
                    result.state = ArtifactState::Unsupported;
                    if parts.len() == 3
                        && matches!(parts[0], "100644" | "100755")
                        && parts[1] == "blob"
                        && listed_path == path
                    {
                        let size = self
                            .git
                            .text(&["cat-file", "-s", parts[2]])
                            .await?
                            .and_then(|s| s.parse::<usize>().ok())
                            .ok_or(LocalGitError::RepositoryUnavailable)?;
                        if size > self.config.max_blob_bytes || size > *remaining {
                            result.state = ArtifactState::TooLarge;
                        } else {
                            let output =
                                self.git.run(&["cat-file", "blob", parts[2]], size).await?;
                            if output.code != 0 || output.bytes.len() != size {
                                return Err(LocalGitError::RepositoryUnavailable);
                            }
                            *remaining -= size;
                            let hash = digest(&output.bytes);
                            result.state =
                                if hash == entry.sha256 && size.to_string() == entry.byte_length {
                                    ArtifactState::Passed
                                } else {
                                    ArtifactState::Mismatch
                                };
                            result.observed_sha256 = Some(hash);
                            result.observed_byte_length = Some(size.to_string());
                        }
                    }
                }
            }
            observed.push(result);
        }
        Ok(observed)
    }

    pub(super) async fn observe(
        &self,
        candidate: &DeliveryCandidate,
        inspection_id: &str,
        binding_digest: String,
    ) -> Result<LocalGitReport, LocalGitError> {
        let expected_format = if self.format == RevisionFormat::GitSha1 {
            "sha1"
        } else {
            "sha256"
        };
        if self
            .git
            .text(&["rev-parse", "--is-bare-repository"])
            .await?
            .as_deref()
            != Some("true")
            || self
                .git
                .text(&["rev-parse", "--show-object-format"])
                .await?
                .as_deref()
                != Some(expected_format)
        {
            return Err(LocalGitError::RepositoryUnavailable);
        }
        let partial = self
            .git
            .run(
                &[
                    "config",
                    "--local",
                    "--includes",
                    "--get-regexp",
                    "^(extensions[.]partialclone|remote[.].*[.]promisor)$",
                ],
                4096,
            )
            .await?;
        if partial.code != 1 {
            return Err(LocalGitError::RepositoryUnavailable);
        }
        let source = candidate.binding.source_revision.as_ref();
        let mut remaining = self.config.max_total_blob_bytes;
        let source_available = if let Some(source) = source {
            source.format == self.format
                && self
                    .git
                    .text(&["cat-file", "-t", &source.value])
                    .await?
                    .as_deref()
                    == Some("commit")
        } else {
            false
        };
        let source_artifacts = if source_available {
            self.artifacts(&candidate.manifest, &source.unwrap().value, &mut remaining)
                .await?
        } else {
            vec![]
        };
        let manifest_outcome = if !source_available {
            VerificationOutcome::Unknown
        } else if source_artifacts
            .iter()
            .all(|a| a.state == ArtifactState::Passed)
        {
            VerificationOutcome::Passed
        } else if source_artifacts
            .iter()
            .any(|a| a.state == ArtifactState::Mismatch)
        {
            VerificationOutcome::Failed
        } else {
            VerificationOutcome::Unknown
        };
        let target_revision = self.target().await?;
        let graph_contains_source = if source_available && target_revision.is_some() {
            match self
                .git
                .run(
                    &[
                        "merge-base",
                        "--is-ancestor",
                        &source.unwrap().value,
                        &target_revision.as_ref().unwrap().value,
                    ],
                    4096,
                )
                .await?
                .code
            {
                0 => Some(true),
                1 => Some(false),
                _ => None,
            }
        } else {
            None
        };
        let target_artifacts = if graph_contains_source == Some(true)
            && manifest_outcome == VerificationOutcome::Passed
        {
            self.artifacts(
                &candidate.manifest,
                &target_revision.as_ref().unwrap().value,
                &mut remaining,
            )
            .await?
        } else {
            vec![]
        };
        let target_stable = target_revision == self.target().await?;
        let integration_outcome = if !target_stable {
            IntegrationOutcome::Unknown
        } else if graph_contains_source == Some(true)
            && manifest_outcome == VerificationOutcome::Passed
            && target_artifacts
                .iter()
                .all(|a| a.state == ArtifactState::Passed)
        {
            IntegrationOutcome::Applied
        } else if target_revision.is_none() || graph_contains_source == Some(false) {
            IntegrationOutcome::Pending
        } else {
            IntegrationOutcome::Unknown
        };
        let target_precondition_matches = match &candidate.binding.target.precondition {
            TargetPrecondition::Missing => target_revision.is_none(),
            TargetPrecondition::Exact(expected) => target_revision.as_ref() == Some(expected),
        };
        let observed_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| LocalGitError::RepositoryUnavailable)?
            .as_millis() as u64;
        let report = LocalGitReport {
            version: 1,
            adapter_id: self.config.adapter_id.clone(),
            inspection_id: inspection_id.into(),
            config_digest: self.config_digest.clone(),
            binding_digest,
            observed_at_unix_ms,
            source_revision: source.cloned(),
            source_artifacts,
            manifest_outcome,
            unsupported_required_checks: candidate
                .binding
                .required_checks
                .iter()
                .filter(|s| s.as_str() != MANIFEST_CHECK)
                .cloned()
                .collect(),
            target_revision,
            target_artifacts,
            graph_contains_source,
            target_stable,
            target_precondition_matches,
            integration_outcome,
        };
        if serde_json::to_vec(&report)
            .map_err(|_| LocalGitError::ReportUnavailable)?
            .len()
            > REPORT_LIMIT
        {
            return Err(LocalGitError::OutputLimit);
        }
        Ok(report)
    }
}
