//! Optional GitHub observations translated into provider-neutral delivery facts.
use super::{
    LocalGitError, REPORT_LIMIT, digest,
    github_http::{GitHubTransport, HttpsTransport},
    local_git::{ArtifactObservation, ArtifactState},
    publish, read_file, text,
};
use awr_team::delivery::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Semaphore;
use url::Url;
use zeroize::Zeroizing;

#[path = "github_content.rs"]
mod content;

pub const MANIFEST_CHECK: &str = "github.manifest";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitHubError {
    InvalidConfiguration,
    BindingMismatch,
    ProviderUnavailable,
    ProviderAuthorizationUnavailable,
    ProviderPolicyUnsupported,
    UnsupportedGuarantee,
    RateLimited,
    InvalidResponse,
    TimedOut,
    OutputLimit,
    ReportUnavailable,
    ReportConflict,
    AuthorizationUnavailable,
    PreconditionsChanged,
    IdempotencyConflict,
    Contention,
    StoreUnavailable,
    DomainRejected,
}

impl From<LocalGitError> for GitHubError {
    fn from(e: LocalGitError) -> Self {
        match e {
            LocalGitError::ReportConflict => Self::ReportConflict,
            LocalGitError::OutputLimit => Self::OutputLimit,
            _ => Self::ReportUnavailable,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubCheckMapping {
    pub check: String,
    pub name: String,
    /// A name alone does not establish the check producer.
    pub app_id: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubConfig {
    pub version: u32,
    pub adapter_id: String,
    pub connector_id: String,
    pub tenant_id: String,
    pub project_id: String,
    pub workstream_id: String,
    pub work_id: String,
    pub resource: String,
    /// HTTPS origin, optionally with an Enterprise API path such as /api/v3.
    pub api_base_url: String,
    pub repository_id: u64,
    pub owner: String,
    pub repository: String,
    pub target_branch: String,
    /// This first adapter observes same-repository PRs only; PR approval is not queried.
    pub pull_number: Option<u64>,
    pub checks: Vec<GitHubCheckMapping>,
    pub report_directory: PathBuf,
    pub request_timeout_ms: u64,
    pub inspection_timeout_ms: u64,
    pub max_response_bytes: usize,
    pub max_total_response_bytes: usize,
    pub max_blob_bytes: usize,
    pub max_total_blob_bytes: usize,
    pub max_checks: usize,
    pub max_requests: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubCheckObservation {
    pub check: String,
    pub app_id: Option<u64>,
    pub run_id: Option<u64>,
    pub outcome: VerificationOutcome,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubPullObservation {
    pub number: u64,
    pub head: String,
    pub base: String,
    pub closed: bool,
    pub merged: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubReport {
    pub version: u32,
    pub adapter_id: String,
    pub inspection_id: String,
    pub config_digest: String,
    pub binding_digest: String,
    pub observed_at_unix_ms: u64,
    pub repository_id: u64,
    pub source_revision: RevisionRef,
    pub source_artifacts: Vec<ArtifactObservation>,
    pub manifest_outcome: VerificationOutcome,
    pub checks: Vec<GitHubCheckObservation>,
    pub unsupported_required_checks: Vec<String>,
    pub pull: Option<GitHubPullObservation>,
    pub pull_stable: bool,
    pub target_revision: Option<RevisionRef>,
    pub target_artifacts: Vec<ArtifactObservation>,
    pub graph_contains_source: Option<bool>,
    pub target_stable: bool,
    pub target_precondition_matches: bool,
    pub integration_outcome: IntegrationOutcome,
    /// Historical reports retain their exact bytes and never invent a witness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_witness: Option<IntegrationContentWitness>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubSnapshot {
    pub report: GitHubReport,
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

#[derive(Clone)]
pub struct GitHubAdapter {
    pub(super) config: GitHubConfig,
    pub(super) config_digest: String,
    api: Arc<dyn GitHubTransport>,
    gate: Arc<Semaphore>,
}

fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

fn path_parts(s: &str) -> Option<Vec<&str>> {
    (text(s, 1024)
        && !s.contains(['\\', ':', '?', '#'])
        && s.split('/').count() <= 16
        && s.split('/').all(|p| !p.is_empty() && p != "." && p != ".."))
    .then(|| s.split('/').collect())
}

fn sha(value: &Value) -> Result<String, GitHubError> {
    let s = value.as_str().ok_or(GitHubError::InvalidResponse)?;
    if s.len() != 40
        || !s
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(GitHubError::InvalidResponse);
    }
    Ok(s.into())
}

impl GitHubAdapter {
    /// Configuration is operator-owned. Opening an adapter makes no network request.
    pub fn open(
        config: GitHubConfig,
        credential: Option<Zeroizing<String>>,
    ) -> Result<Self, GitHubError> {
        Self::from_transport(config, Arc::new(HttpsTransport::new(credential)?))
    }

    /// Trusted library injection, unavailable through the HTTP/MCP request surface.
    pub fn from_transport(
        mut config: GitHubConfig,
        api: Arc<dyn GitHubTransport>,
    ) -> Result<Self, GitHubError> {
        let base =
            Url::parse(&config.api_base_url).map_err(|_| GitHubError::InvalidConfiguration)?;
        if config.version != 1
            || base.scheme() != "https"
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || (base.path() != "/" && base.path() != "/api/v3" && base.path() != "/api/v3/")
            || config.repository_id == 0
            || !identifier(&config.owner)
            || !identifier(&config.repository)
            || path_parts(&config.target_branch).is_none()
            || config.target_branch.contains([' ', '~', '^', '*', '['])
            || config.target_branch.contains("..")
            || config.target_branch.contains("@{")
            || config
                .target_branch
                .split('/')
                .any(|p| p.starts_with('.') || p.ends_with('.') || p.ends_with(".lock"))
            || config.pull_number == Some(0)
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
            || !(100..=10000).contains(&config.request_timeout_ms)
            || !(config.request_timeout_ms..=60000).contains(&config.inspection_timeout_ms)
            || !(1024..=16 * 1024 * 1024).contains(&config.max_response_bytes)
            || !(config.max_response_bytes..=64 * 1024 * 1024)
                .contains(&config.max_total_response_bytes)
            || !(1..=8 * 1024 * 1024).contains(&config.max_blob_bytes)
            || !(config.max_blob_bytes..=32 * 1024 * 1024).contains(&config.max_total_blob_bytes)
            || !(1..=2000).contains(&config.max_checks)
            || !(1..=2048).contains(&config.max_requests)
            || config.checks.len() > 28
            || !config.report_directory.is_absolute()
        {
            return Err(GitHubError::InvalidConfiguration);
        }
        let mut keys = BTreeSet::new();
        let mut producers = BTreeSet::new();
        for c in &config.checks {
            if !text(&c.check, 128)
                || c.check == MANIFEST_CHECK
                || !text(&c.name, 128)
                || c.app_id == 0
                || !keys.insert(&c.check)
                || !producers.insert((&c.name, c.app_id))
            {
                return Err(GitHubError::InvalidConfiguration);
            }
        }
        config.api_base_url = base.as_str().trim_end_matches('/').into();
        config.report_directory = fs::canonicalize(&config.report_directory)
            .map_err(|_| GitHubError::InvalidConfiguration)?;
        if !config.report_directory.is_dir() {
            return Err(GitHubError::InvalidConfiguration);
        }
        let config_digest =
            digest(&serde_json::to_vec(&config).map_err(|_| GitHubError::InvalidConfiguration)?);
        Ok(Self {
            config,
            config_digest,
            api,
            gate: Arc::new(Semaphore::new(1)),
        })
    }

    pub fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            adapter_id: self.config.adapter_id.clone(),
            inspection: true,
            change_requests: self.config.pull_number.is_some(),
            verification: true,
            integration_requests: false,
            integration_observations: true,
            notifications: false,
            polling: true,
        }
    }

    pub(super) fn validate_candidate(&self, c: &DeliveryCandidate) -> Result<(), GitHubError> {
        DeliveryRecord::Candidate(c.clone())
            .validate()
            .map_err(|_| GitHubError::BindingMismatch)?;
        let b = &c.binding;
        let reference = format!("refs/heads/{}", self.config.target_branch);
        if b.tenant_id.as_str() != self.config.tenant_id
            || b.project_id.as_str() != self.config.project_id
            || b.scope_id.as_str() != "main"
            || b.work_id.as_str() != self.config.work_id
            || b.workstream_id != self.config.workstream_id
            || b.target.resource != self.config.resource
            || b.target.reference.as_deref() != Some(&reference)
            || !b.source_revision.as_ref().is_some_and(|s| {
                s.resource == self.config.resource && s.format == RevisionFormat::GitSha1
            })
            || matches!(&b.target.precondition, TargetPrecondition::Exact(s) if s.format != RevisionFormat::GitSha1)
        {
            return Err(GitHubError::BindingMismatch);
        }
        Ok(())
    }

    /// The blocking HTTPS job never publishes files. Dropping the future cannot
    /// leave a late report; its bounded job retains the semaphore until it exits.
    pub(super) async fn probe(
        &self,
        c: &DeliveryCandidate,
        inspection: &str,
    ) -> Result<GitHubReport, GitHubError> {
        self.probe_mode(c, inspection, true).await
    }

    /// Original-effect recovery proves immutable content and actual inclusion.
    /// A changed PR head or new check cannot replace that historical request.
    pub(super) async fn probe_original(
        &self,
        c: &DeliveryCandidate,
        inspection: &str,
    ) -> Result<GitHubReport, GitHubError> {
        self.probe_mode(c, inspection, false).await
    }

    async fn probe_mode(
        &self,
        c: &DeliveryCandidate,
        inspection: &str,
        current_pr_and_checks: bool,
    ) -> Result<GitHubReport, GitHubError> {
        self.validate_candidate(c)?;
        let deadline = Instant::now() + Duration::from_millis(self.config.inspection_timeout_ms);
        let permit = tokio::time::timeout_at(deadline.into(), self.gate.clone().acquire_owned())
            .await
            .map_err(|_| GitHubError::TimedOut)?
            .map_err(|_| GitHubError::ProviderUnavailable)?;
        let adapter = self.clone();
        let candidate = c.clone();
        let inspection = inspection.to_owned();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            Query::new(&adapter, deadline).observe(&candidate, &inspection, current_pr_and_checks)
        })
        .await
        .map_err(|_| GitHubError::ProviderUnavailable)?
    }

    /// Fresh, unpublished permission/policy/source/check proof before a permit
    /// is rechecked in the actual store. No REST merge or reference mutation.
    pub(super) async fn integration_preflight(
        &self,
        c: &DeliveryCandidate,
    ) -> Result<GitHubReport, GitHubError> {
        self.validate_candidate(c)?;
        if !matches!(c.binding.target.precondition, TargetPrecondition::Exact(_)) {
            return Err(GitHubError::UnsupportedGuarantee);
        }
        let deadline = Instant::now() + Duration::from_millis(self.config.inspection_timeout_ms);
        let permit = tokio::time::timeout_at(deadline.into(), self.gate.clone().acquire_owned())
            .await
            .map_err(|_| GitHubError::TimedOut)?
            .map_err(|_| GitHubError::ProviderUnavailable)?;
        let adapter = self.clone();
        let candidate = c.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut query = Query::new(&adapter, deadline);
            query.integration_policy(&candidate)?;
            let report = query.observe(&candidate, "integration-preflight", true)?;
            if report.manifest_outcome != VerificationOutcome::Passed
                || !report.target_stable
                || !report.target_precondition_matches
                || !report.pull_stable
                || !report.unsupported_required_checks.is_empty()
                || report
                    .checks
                    .iter()
                    .any(|c| c.outcome != VerificationOutcome::Passed)
            {
                return Err(GitHubError::PreconditionsChanged);
            }
            // Recheck policy and pinned identity after the potentially longer
            // content/check reads. Receive-pack enforces the live server policy.
            query.integration_policy(&candidate)?;
            Ok(report)
        })
        .await
        .map_err(|_| GitHubError::ProviderUnavailable)?
    }

    pub async fn inspect(
        &self,
        c: &DeliveryCandidate,
        inspection: &str,
    ) -> Result<GitHubSnapshot, GitHubError> {
        self.validate_candidate(c)?;
        if !text(inspection, 128) {
            return Err(GitHubError::BindingMismatch);
        }
        let index = self.inspection_index(inspection);
        if let Some(s) = self.cached(&index, c, inspection)? {
            return Ok(s);
        }
        let report = self.probe(c, inspection).await?;
        let bytes = serde_json::to_vec(&report).map_err(|_| GitHubError::ReportUnavailable)?;
        let report_sha256 = digest(&bytes);
        publish(
            &self
                .config
                .report_directory
                .join(format!("github-report-{report_sha256}.json")),
            &bytes,
        )?;
        let pointer = InspectionIndex {
            config_digest: self.config_digest.clone(),
            binding_digest: c
                .binding
                .digest()
                .map_err(|_| GitHubError::BindingMismatch)?,
            report_sha256,
        };
        match publish(&index, &serde_json::to_vec(&pointer).unwrap()) {
            Ok(()) | Err(LocalGitError::ReportConflict) => self
                .cached(&index, c, inspection)?
                .ok_or(GitHubError::ReportUnavailable),
            Err(e) => Err(e.into()),
        }
    }

    pub fn report_bytes(&self, hash: &str) -> Result<Vec<u8>, GitHubError> {
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(GitHubError::ReportUnavailable);
        }
        let bytes = read_file(
            &self
                .config
                .report_directory
                .join(format!("github-report-{hash}.json")),
        )?
        .ok_or(GitHubError::ReportUnavailable)?;
        if digest(&bytes) != hash {
            return Err(GitHubError::ReportConflict);
        }
        Ok(bytes)
    }

    pub(super) fn inspection_index(&self, inspection: &str) -> PathBuf {
        let key = digest(&serde_json::to_vec(&(&self.config.adapter_id, inspection)).unwrap());
        self.config
            .report_directory
            .join(format!("github-inspection-{key}.json"))
    }

    pub(super) fn cached(
        &self,
        index: &Path,
        c: &DeliveryCandidate,
        inspection: &str,
    ) -> Result<Option<GitHubSnapshot>, GitHubError> {
        let Some(bytes) = read_file(index)? else {
            return Ok(None);
        };
        let pointer: InspectionIndex =
            serde_json::from_slice(&bytes).map_err(|_| GitHubError::ReportUnavailable)?;
        if pointer.config_digest != self.config_digest
            || pointer.binding_digest != c.binding.digest().unwrap()
        {
            return Err(GitHubError::ReportConflict);
        }
        let bytes = self.report_bytes(&pointer.report_sha256)?;
        let report: GitHubReport =
            serde_json::from_slice(&bytes).map_err(|_| GitHubError::ReportUnavailable)?;
        if report.version != 1
            || report.config_digest != self.config_digest
            || report.binding_digest != pointer.binding_digest
            || report.inspection_id != inspection
            || report.adapter_id != self.config.adapter_id
        {
            return Err(GitHubError::ReportConflict);
        }
        let artifact = ArtifactEntry {
            artifact_id: format!("github-report-{}", pointer.report_sha256),
            sha256: pointer.report_sha256,
            byte_length: bytes.len().to_string(),
            locator: format!(
                "awr-github-report:{}:{}",
                self.config.adapter_id,
                digest(&bytes)
            ),
        };
        let provenance = FactProvenance {
            source: FactSource::AdapterObservation,
            reference: artifact.locator.clone(),
            observed_at_unix_ms: Some(report.observed_at_unix_ms),
            recorded_at_unix_ms: report.observed_at_unix_ms,
        };
        let wrap = |record| DeliveryEnvelope {
            protocol: DELIVERY_PROTOCOL.into(),
            protocol_version: DELIVERY_PROTOCOL_VERSION,
            record,
        };
        let verification = |check: String, outcome| {
            wrap(DeliveryRecord::Verification(VerificationRun {
                binding: c.binding.clone(),
                run_id: format!(
                    "github-{}-{}",
                    digest(check.as_bytes()),
                    &artifact.sha256[..32]
                ),
                check,
                outcome,
                result_artifact: Some(artifact.clone()),
                provenance: provenance.clone(),
            }))
        };
        let mut records = vec![verification(
            MANIFEST_CHECK.into(),
            report.manifest_outcome.clone(),
        )];
        records.extend(
            report
                .checks
                .iter()
                .map(|check| verification(check.check.clone(), check.outcome.clone())),
        );
        if let Some(pull) = &report.pull {
            records.push(wrap(DeliveryRecord::ChangeRequest(ChangeRequest {
                binding: c.binding.clone(),
                provider: "github".into(),
                resource_id: format!("{}:{}", self.config.repository_id, pull.number),
                locator: Some(artifact.locator.clone()),
                provenance: provenance.clone(),
            })));
        }
        let observation = IntegrationObservation {
            binding: c.binding.clone(),
            request_id: None,
            external_reference: format!(
                "github-target:{}:{}",
                self.config.repository_id, self.config.target_branch
            ),
            outcome: report.integration_outcome.clone(),
            result_revision: report.target_revision.clone(),
            contains_manifest_digest: (report.integration_outcome == IntegrationOutcome::Applied)
                .then(|| c.binding.manifest_digest.clone()),
            provenance,
        };
        records.push(wrap(DeliveryRecord::IntegrationObservation(
            observation.clone(),
        )));
        if let Some(witness) = &report.content_witness {
            records.push(content_record(&observation, witness.clone()));
        }
        for r in &records {
            r.validate().map_err(|_| GitHubError::ReportConflict)?;
        }
        Ok(Some(GitHubSnapshot {
            report,
            report_artifact: artifact,
            records,
        }))
    }
}

struct Query<'a> {
    adapter: &'a GitHubAdapter,
    deadline: Instant,
    requests: usize,
    remaining_blobs: usize,
    remaining_responses: usize,
    trees: BTreeMap<String, Value>,
}

impl<'a> Query<'a> {
    fn new(adapter: &'a GitHubAdapter, deadline: Instant) -> Self {
        Self {
            adapter,
            deadline,
            requests: 0,
            remaining_blobs: adapter.config.max_total_blob_bytes,
            remaining_responses: adapter.config.max_total_response_bytes,
            trees: BTreeMap::new(),
        }
    }

    fn get(
        &mut self,
        tail: &[&str],
        query: &[(&str, String)],
    ) -> Result<Option<Value>, GitHubError> {
        if self.requests >= self.adapter.config.max_requests {
            return Err(GitHubError::OutputLimit);
        }
        let remaining = self
            .deadline
            .checked_duration_since(Instant::now())
            .ok_or(GitHubError::TimedOut)?;
        let mut url = Url::parse(&self.adapter.config.api_base_url).unwrap();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| GitHubError::InvalidConfiguration)?;
            segments
                .pop_if_empty()
                .extend([
                    "repos",
                    &self.adapter.config.owner,
                    &self.adapter.config.repository,
                ])
                .extend(tail);
        }
        if !query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(query.iter().map(|(k, v)| (*k, v)));
        }
        self.requests += 1;
        let limit = self
            .adapter
            .config
            .max_response_bytes
            .min(self.remaining_responses);
        if limit == 0 {
            return Err(GitHubError::OutputLimit);
        }
        let response = self.adapter.api.get(
            &url,
            remaining.min(Duration::from_millis(
                self.adapter.config.request_timeout_ms,
            )),
            limit,
        )?;
        if Instant::now() >= self.deadline {
            return Err(GitHubError::TimedOut);
        }
        if response.body.len() > limit {
            return Err(GitHubError::OutputLimit);
        }
        self.remaining_responses -= response.body.len();
        match response.status {
            200 => serde_json::from_slice(&response.body)
                .map(Some)
                .map_err(|_| GitHubError::InvalidResponse),
            404 => Ok(None),
            401 | 403 => Err(GitHubError::ProviderAuthorizationUnavailable),
            429 => Err(GitHubError::RateLimited),
            _ => Err(GitHubError::ProviderUnavailable),
        }
    }

    fn required(&mut self, tail: &[&str]) -> Result<Value, GitHubError> {
        self.get(tail, &[])?.ok_or(GitHubError::ProviderUnavailable)
    }

    fn repository(&mut self) -> Result<Value, GitHubError> {
        let repo = self.required(&[])?;
        if repo["id"].as_u64() != Some(self.adapter.config.repository_id)
            || !repo["full_name"].as_str().is_some_and(|s| {
                s.eq_ignore_ascii_case(&format!(
                    "{}/{}",
                    self.adapter.config.owner, self.adapter.config.repository
                ))
            })
        {
            return Err(GitHubError::BindingMismatch);
        }
        Ok(repo)
    }

    fn integration_policy(&mut self, c: &DeliveryCandidate) -> Result<(), GitHubError> {
        let repo = self.repository()?;
        if repo["permissions"]["push"] != true {
            return Err(GitHubError::ProviderAuthorizationUnavailable);
        }
        if repo["archived"] != false {
            return Err(GitHubError::ProviderPolicyUnsupported);
        }
        let TargetPrecondition::Exact(expected) = &c.binding.target.precondition else {
            return Err(GitHubError::UnsupportedGuarantee);
        };
        let branch = self.adapter.config.target_branch.clone();
        let current = self.required(&["branches", &branch])?;
        if current["name"] != branch || sha(&current["commit"]["sha"])? != expected.value {
            return Err(GitHubError::PreconditionsChanged);
        }
        if current["protected"] != false {
            return Err(GitHubError::ProviderPolicyUnsupported);
        }
        let rules = self
            .get(&["rules", "branches", &branch], &[])?
            .ok_or(GitHubError::ProviderPolicyUnsupported)?;
        if rules.as_array().is_none_or(|r| !r.is_empty()) {
            return Err(GitHubError::ProviderPolicyUnsupported);
        }
        let source = c.binding.source_revision.as_ref().unwrap();
        let comparison = self.comparison(&expected.value, &source.value)?;
        if sha(&comparison["base_commit"]["sha"])? != expected.value
            || sha(&comparison["merge_base_commit"]["sha"])? != expected.value
            || !matches!(comparison["status"].as_str(), Some("ahead" | "identical"))
        {
            return Err(GitHubError::PreconditionsChanged);
        }
        Ok(())
    }

    fn revision(&self, value: String) -> RevisionRef {
        RevisionRef {
            resource: self.adapter.config.resource.clone(),
            format: RevisionFormat::GitSha1,
            value,
        }
    }

    fn target(&mut self) -> Result<Option<RevisionRef>, GitHubError> {
        let branch = self.adapter.config.target_branch.clone();
        let mut tail = vec!["git", "ref", "heads"];
        tail.extend(branch.split('/'));
        let Some(target) = self.get(&tail, &[])? else {
            return Ok(None);
        };
        if target["ref"] != format!("refs/heads/{branch}") || target["object"]["type"] != "commit" {
            return Err(GitHubError::BindingMismatch);
        }
        Ok(Some(self.revision(sha(&target["object"]["sha"])?)))
    }

    fn commit(&mut self, revision: &str) -> Result<String, GitHubError> {
        let commit = self.required(&["git", "commits", revision])?;
        if sha(&commit["sha"])? != revision {
            return Err(GitHubError::BindingMismatch);
        }
        sha(&commit["tree"]["sha"])
    }

    fn pull(&mut self, source: &str) -> Result<Option<GitHubPullObservation>, GitHubError> {
        let Some(number) = self.adapter.config.pull_number else {
            return Ok(None);
        };
        let pull = self.required(&["pulls", &number.to_string()])?;
        if pull["number"].as_u64() != Some(number)
            || pull["head"]["repo"]["id"].as_u64() != Some(self.adapter.config.repository_id)
            || pull["base"]["repo"]["id"].as_u64() != Some(self.adapter.config.repository_id)
            || pull["base"]["ref"] != self.adapter.config.target_branch
            || sha(&pull["head"]["sha"])? != source
        {
            return Err(GitHubError::BindingMismatch);
        }
        let closed = match pull["state"].as_str() {
            Some("open") => false,
            Some("closed") => true,
            _ => return Err(GitHubError::InvalidResponse),
        };
        Ok(Some(GitHubPullObservation {
            number,
            head: source.into(),
            base: sha(&pull["base"]["sha"])?,
            closed,
            merged: pull["merged"]
                .as_bool()
                .ok_or(GitHubError::InvalidResponse)?,
        }))
    }

    fn tree(&mut self, id: &str) -> Result<Value, GitHubError> {
        if let Some(tree) = self.trees.get(id) {
            return Ok(tree.clone());
        }
        let tree = self.required(&["git", "trees", id])?;
        if sha(&tree["sha"])? != id || tree["truncated"] != false || !tree["tree"].is_array() {
            return Err(GitHubError::InvalidResponse);
        }
        self.trees.insert(id.into(), tree.clone());
        Ok(tree)
    }

    fn artifact(
        &mut self,
        entry: &ArtifactEntry,
        root: &str,
    ) -> Result<ArtifactObservation, GitHubError> {
        let mut result = ArtifactObservation {
            artifact_id: entry.artifact_id.clone(),
            state: ArtifactState::Unsupported,
            observed_sha256: None,
            observed_byte_length: None,
        };
        let Some(parts) = entry.locator.strip_prefix("git-blob:").and_then(path_parts) else {
            return Ok(result);
        };
        let mut id = root.to_owned();
        for (index, part) in parts.iter().enumerate() {
            let tree = self.tree(&id)?;
            let entries: Vec<_> = tree["tree"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|e| e["path"] == *part)
                .collect();
            if entries.is_empty() {
                result.state = ArtifactState::Unavailable;
                return Ok(result);
            }
            if entries.len() != 1 {
                return Err(GitHubError::InvalidResponse);
            }
            let item = entries[0];
            id = sha(&item["sha"])?;
            if index + 1 < parts.len() {
                if item["type"] != "tree" || item["mode"] != "040000" {
                    return Ok(result);
                }
                continue;
            }
            if item["type"] != "blob" || !matches!(item["mode"].as_str(), Some("100644" | "100755"))
            {
                return Ok(result);
            }
            let size = item["size"].as_u64().ok_or(GitHubError::InvalidResponse)?;
            if size > self.adapter.config.max_blob_bytes as u64
                || size > self.remaining_blobs as u64
            {
                result.state = ArtifactState::TooLarge;
                return Ok(result);
            }
            let Some(blob) = self.get(&["git", "blobs", &id], &[])? else {
                result.state = ArtifactState::Unavailable;
                return Ok(result);
            };
            if sha(&blob["sha"])? != id
                || blob["size"].as_u64() != Some(size)
                || blob["encoding"] != "base64"
            {
                return Err(GitHubError::InvalidResponse);
            }
            let encoded = blob["content"]
                .as_str()
                .ok_or(GitHubError::InvalidResponse)?;
            let encoded: Vec<_> = encoded
                .bytes()
                .filter(|b| !matches!(b, b'\r' | b'\n'))
                .collect();
            if encoded.len() != (size as usize).div_ceil(3) * 4 {
                return Err(GitHubError::InvalidResponse);
            }
            let bytes = STANDARD
                .decode(&encoded)
                .map_err(|_| GitHubError::InvalidResponse)?;
            if bytes.len() as u64 != size {
                return Err(GitHubError::InvalidResponse);
            }
            self.remaining_blobs -= bytes.len();
            let hash = digest(&bytes);
            result.state = if hash == entry.sha256 && size.to_string() == entry.byte_length {
                ArtifactState::Passed
            } else {
                ArtifactState::Mismatch
            };
            result.observed_sha256 = Some(hash);
            result.observed_byte_length = Some(size.to_string());
        }
        Ok(result)
    }

    fn artifacts(
        &mut self,
        c: &DeliveryCandidate,
        root: &str,
    ) -> Result<Vec<ArtifactObservation>, GitHubError> {
        c.manifest
            .entries
            .iter()
            .map(|e| self.artifact(e, root))
            .collect()
    }

    fn check_outcome(value: &Value) -> VerificationOutcome {
        match (value["status"].as_str(), value["conclusion"].as_str()) {
            (Some("completed"), Some("success")) => VerificationOutcome::Passed,
            (
                Some("completed"),
                Some("failure" | "cancelled" | "timed_out" | "action_required" | "startup_failure"),
            ) => VerificationOutcome::Failed,
            (Some("queued" | "in_progress" | "waiting" | "pending" | "requested"), None) => {
                VerificationOutcome::Running
            }
            _ => VerificationOutcome::Unknown,
        }
    }

    fn checks(
        &mut self,
        c: &DeliveryCandidate,
        source: &str,
    ) -> Result<Vec<GitHubCheckObservation>, GitHubError> {
        let mappings: Vec<_> = self
            .adapter
            .config
            .checks
            .iter()
            .filter(|m| c.binding.required_checks.contains(&m.check))
            .cloned()
            .collect();
        if mappings.is_empty() {
            return Ok(Vec::new());
        }
        let mut runs = Vec::new();
        let mut ids = BTreeSet::new();
        let mut total = None;
        for page in 1..=self.adapter.config.max_checks.div_ceil(100) {
            let response = self
                .get(
                    &["commits", source, "check-runs"],
                    &[
                        ("filter", "latest".into()),
                        ("per_page", "100".into()),
                        ("page", page.to_string()),
                    ],
                )?
                .ok_or(GitHubError::ProviderUnavailable)?;
            let count = response["total_count"]
                .as_u64()
                .ok_or(GitHubError::InvalidResponse)?;
            if count > self.adapter.config.max_checks as u64 {
                return Err(GitHubError::OutputLimit);
            }
            if total.is_some_and(|old| old != count) {
                return Err(GitHubError::InvalidResponse);
            }
            total = Some(count);
            let items = response["check_runs"]
                .as_array()
                .ok_or(GitHubError::InvalidResponse)?;
            if items.len() != (count as usize).saturating_sub(runs.len()).min(100) {
                return Err(GitHubError::InvalidResponse);
            }
            for run in items {
                let id = run["id"]
                    .as_u64()
                    .filter(|i| *i > 0)
                    .ok_or(GitHubError::InvalidResponse)?;
                if !ids.insert(id) || sha(&run["head_sha"])? != source {
                    return Err(GitHubError::InvalidResponse);
                }
                runs.push(run.clone());
            }
            if runs.len() as u64 == count {
                break;
            }
        }
        let mut result = Vec::new();
        for mapping in mappings {
            let matching: Vec<_> = runs
                .iter()
                .filter(|r| {
                    r["name"] == mapping.name && r["app"]["id"].as_u64() == Some(mapping.app_id)
                })
                .collect();
            let mut observed = GitHubCheckObservation {
                check: mapping.check,
                app_id: Some(mapping.app_id),
                run_id: None,
                outcome: VerificationOutcome::Unknown,
            };
            if matching.len() == 1 {
                let run = matching[0];
                let id = run["id"].as_u64().unwrap();
                let current = self.required(&["check-runs", &id.to_string()])?;
                observed.run_id = Some(id);
                if current["id"] == run["id"]
                    && current["head_sha"] == source
                    && current["name"] == run["name"]
                    && current["app"]["id"] == run["app"]["id"]
                    && current["status"] == run["status"]
                    && current["conclusion"] == run["conclusion"]
                {
                    observed.outcome = Self::check_outcome(&current);
                }
            }
            result.push(observed);
        }
        Ok(result)
    }

    fn observe(
        &mut self,
        c: &DeliveryCandidate,
        inspection: &str,
        current_pr_and_checks: bool,
    ) -> Result<GitHubReport, GitHubError> {
        self.repository()?;
        let source = c.binding.source_revision.as_ref().unwrap();
        let pull = if current_pr_and_checks {
            self.pull(&source.value)?
        } else {
            None
        };
        let root = self.commit(&source.value)?;
        let source_artifacts = self.artifacts(c, &root)?;
        let manifest_outcome = if source_artifacts
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
        let mut checks = if current_pr_and_checks {
            self.checks(c, &source.value)?
        } else {
            Vec::new()
        };
        let target = self.target()?;
        let contains = if let Some(target) = &target {
            Some(self.contains(&source.value, &target.value)?)
        } else {
            None
        };
        let mut content_witness = self.content_witness(c, &root, target.as_ref())?;
        let usable_content = !matches!(
            content_witness,
            IntegrationContentWitness::Unavailable { .. }
        );
        let target_artifacts = if (contains == Some(true) || usable_content)
            && manifest_outcome == VerificationOutcome::Passed
        {
            let root = self.commit(&target.as_ref().unwrap().value)?;
            self.artifacts(c, &root)?
        } else {
            Vec::new()
        };
        let pull_stable = !current_pr_and_checks || pull == self.pull(&source.value)?;
        self.repository()?;
        // The final ref barrier follows every proof, artifact and PR read.
        let target_stable = target == self.target()?;
        if !target_stable || !pull_stable {
            content_witness = IntegrationContentWitness::Unavailable {
                reason: ContentProofUnavailableReason::TargetUnstable,
            };
        }
        // A PR that moved while checking cannot carry a terminal verification.
        let manifest_outcome = if pull_stable {
            manifest_outcome
        } else {
            VerificationOutcome::Unknown
        };
        if !pull_stable {
            for check in &mut checks {
                check.outcome = VerificationOutcome::Unknown;
            }
        }
        let integration_outcome = if !target_stable || !pull_stable {
            IntegrationOutcome::Unknown
        } else if usable_content
            && manifest_outcome == VerificationOutcome::Passed
            && target_artifacts
                .iter()
                .all(|a| a.state == ArtifactState::Passed)
        {
            IntegrationOutcome::Applied
        } else if target.is_none() || contains == Some(false) {
            IntegrationOutcome::Pending
        } else {
            IntegrationOutcome::Unknown
        };
        let precondition = match &c.binding.target.precondition {
            TargetPrecondition::Missing => target.is_none(),
            TargetPrecondition::Exact(e) => target.as_ref() == Some(e),
        };
        let report = GitHubReport {
            version: 1,
            adapter_id: self.adapter.config.adapter_id.clone(),
            inspection_id: inspection.into(),
            config_digest: self.adapter.config_digest.clone(),
            binding_digest: c.binding.digest().unwrap(),
            observed_at_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| GitHubError::ProviderUnavailable)?
                .as_millis() as u64,
            repository_id: self.adapter.config.repository_id,
            source_revision: source.clone(),
            source_artifacts,
            manifest_outcome,
            checks,
            unsupported_required_checks: c
                .binding
                .required_checks
                .iter()
                .filter(|s| {
                    s.as_str() != MANIFEST_CHECK
                        && !self.adapter.config.checks.iter().any(|m| &m.check == *s)
                })
                .cloned()
                .collect(),
            pull,
            pull_stable,
            target_revision: target,
            target_artifacts,
            graph_contains_source: contains,
            target_stable,
            target_precondition_matches: precondition,
            integration_outcome,
            content_witness: Some(content_witness),
        };
        if serde_json::to_vec(&report)
            .map_err(|_| GitHubError::ReportUnavailable)?
            .len()
            > REPORT_LIMIT
        {
            return Err(GitHubError::OutputLimit);
        }
        Ok(report)
    }
}

pub(super) fn content_record(
    observation: &IntegrationObservation,
    witness: IntegrationContentWitness,
) -> DeliveryEnvelope {
    DeliveryEnvelope {
        protocol: DELIVERY_PROTOCOL.into(),
        protocol_version: DELIVERY_PROTOCOL_VERSION,
        record: DeliveryRecord::IntegrationContentProof(IntegrationContentProof {
            binding: observation.binding.clone(),
            request_id: observation.request_id.clone(),
            observation_reference: observation.external_reference.clone(),
            result_revision: observation.result_revision.clone(),
            witness,
            provenance: observation.provenance.clone(),
        }),
    }
}
