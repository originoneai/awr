//! Opt-in guarded HTTPS Git effects. Recovery observes the original request only.
use super::github_http::GitHubTransport;
use super::github_receive::{
    self, GitHubReceiveMethod, GitHubReceiveTransport, HttpsReceivePack, PushStatus,
};
use super::{
    GitHubAdapter, GitHubConfig, GitHubError, GitHubReport, REPORT_LIMIT, digest, publish,
    publish_once, read_file, text,
};
use awr_team::delivery::*;
use awr_team_pg::{
    ConfirmDeliveryIntegration, DeliveryIntegrationPermit, DeliveryReadSet, DeliveryScheduleQuery,
    DeliverySyncStore, DispatchDeliveryIntegration, IngestDeliveryFacts, ReserveDeliveryInspection,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;
use zeroize::Zeroizing;

fn domain_error(error: awr_team_pg::PgError) -> GitHubError {
    use super::LocalGitError;
    match super::domain_error(error) {
        LocalGitError::AuthorizationUnavailable => GitHubError::AuthorizationUnavailable,
        LocalGitError::PreconditionsChanged => GitHubError::PreconditionsChanged,
        LocalGitError::IdempotencyConflict => GitHubError::IdempotencyConflict,
        LocalGitError::Contention => GitHubError::Contention,
        LocalGitError::DomainRejected => GitHubError::DomainRejected,
        _ => GitHubError::StoreUnavailable,
    }
}

struct LaunchGuard(Arc<AtomicU8>);
impl LaunchGuard {
    fn new() -> Self {
        Self(Arc::new(AtomicU8::new(0)))
    }
    fn cancel(&self) {
        let _ = self
            .0
            .compare_exchange(0, 2, Ordering::SeqCst, Ordering::SeqCst);
    }
    fn failed(&self, diagnostic: GitHubError) -> GitHubAttemptOutcome {
        self.cancel();
        if self.0.load(Ordering::SeqCst) == 1 {
            GitHubAttemptOutcome::Uncertain { diagnostic }
        } else {
            GitHubAttemptOutcome::NotStarted { diagnostic }
        }
    }
}
impl Drop for LaunchGuard {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubIntegrationConfig {
    pub enabled: bool,
    pub repository: GitHubConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubIntegrationPollRequest {
    /// Retry this ID for the same immutable observation; use a new ID to requery.
    pub request_id: String,
    pub integration_id: String,
    /// Current admission, distinct from the original candidate's historical set.
    pub read_set: DeliveryReadSet,
    pub connector_version: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum GitHubAttemptOutcome {
    NotStarted { diagnostic: GitHubError },
    Accepted,
    Rejected,
    Uncertain { diagnostic: GitHubError },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubIntegrationReport {
    pub version: u32,
    pub config_digest: String,
    pub candidate_digest: String,
    pub request_digest: String,
    pub integration_id: String,
    pub inspection_id: String,
    pub attempt_recorded: bool,
    pub command_outcome: Option<GitHubAttemptOutcome>,
    pub observation: Option<GitHubReport>,
    pub diagnostic: Option<GitHubError>,
    pub outcome: IntegrationOutcome,
    pub observed_at_unix_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubIntegrationSnapshot {
    pub report: GitHubIntegrationReport,
    pub report_artifact: ArtifactEntry,
    pub records: Vec<DeliveryEnvelope>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Attempt {
    version: u32,
    config_digest: String,
    candidate: DeliveryCandidate,
    request: IntegrationRequest,
    read_set: DeliveryReadSet,
    connector_version: String,
    eligibility_digest: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandResult {
    request_digest: String,
    outcome: GitHubAttemptOutcome,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportIndex {
    request_digest: String,
    report_sha256: String,
}

fn bytes(value: &impl Serialize) -> Result<Vec<u8>, GitHubError> {
    let encoded = serde_json::to_vec(value).map_err(|_| GitHubError::ReportUnavailable)?;
    if encoded.len() > REPORT_LIMIT {
        return Err(GitHubError::OutputLimit);
    }
    Ok(encoded)
}

pub struct GitHubIntegrator {
    observer: GitHubAdapter,
    receive: Arc<dyn GitHubReceiveTransport>,
    gate: Arc<Semaphore>,
}

impl GitHubIntegrator {
    /// Opt-in mechanical support alone grants no AWR authority.
    pub async fn open(
        config: GitHubIntegrationConfig,
        credential: Zeroizing<String>,
    ) -> Result<Self, GitHubError> {
        if !config.enabled {
            return Err(GitHubError::InvalidConfiguration);
        }
        let receive = Arc::new(HttpsReceivePack::new(credential.clone())?);
        let observer = GitHubAdapter::open(config.repository, Some(credential))?;
        github_receive::urls(&observer.config)?;
        Ok(Self {
            observer,
            receive,
            gate: Arc::new(Semaphore::new(1)),
        })
    }

    /// Trusted operator fixture/transport extension; public receipts still grant no effect.
    pub fn from_transports(
        config: GitHubIntegrationConfig,
        api: Arc<dyn GitHubTransport>,
        receive: Arc<dyn GitHubReceiveTransport>,
    ) -> Result<Self, GitHubError> {
        if !config.enabled {
            return Err(GitHubError::InvalidConfiguration);
        }
        let observer = GitHubAdapter::from_transport(config.repository, api)?;
        github_receive::urls(&observer.config)?;
        Ok(Self {
            observer,
            receive,
            gate: Arc::new(Semaphore::new(1)),
        })
    }

    pub fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            integration_requests: true,
            ..self.observer.capabilities()
        }
    }

    fn paths(&self, id: &str) -> (PathBuf, PathBuf) {
        let config = &self.observer.config;
        let key =
            digest(&serde_json::to_vec(&(&config.tenant_id, &config.project_id, id)).unwrap());
        (
            config
                .report_directory
                .join(format!("github-integration-{key}-attempt.json")),
            config
                .report_directory
                .join(format!("github-integration-{key}-result.json")),
        )
    }

    fn validate(&self, attempt: &Attempt) -> Result<(), GitHubError> {
        self.observer.validate_candidate(&attempt.candidate)?;
        DeliveryRecord::IntegrationRequest(attempt.request.clone())
            .validate_against(&attempt.candidate.binding)
            .map_err(|_| GitHubError::BindingMismatch)?;
        let config = &self.observer.config;
        if attempt.version != 1
            || attempt.config_digest != self.observer.config_digest
            || attempt.read_set.work_id != config.work_id
            || attempt.read_set.workstream_id.to_string() != config.workstream_id
            || attempt.read_set.contract_hash != attempt.candidate.binding.contract_hash
        {
            return Err(GitHubError::BindingMismatch);
        }
        Ok(())
    }

    async fn current_mapping(
        &self,
        store: &DeliverySyncStore,
        credential: &str,
    ) -> Result<Value, GitHubError> {
        let config = &self.observer.config;
        let schedule = store
            .schedule(
                &config.tenant_id,
                &config.project_id,
                credential,
                DeliveryScheduleQuery {
                    work_id: config.work_id.clone(),
                    connector_id: config.connector_id.clone(),
                    cursor: None,
                    limit: 1,
                },
            )
            .await
            .map_err(domain_error)?;
        if schedule["connector"]["provider"] != "github"
            || schedule["connector"]["resource"] != config.resource
        {
            return Err(GitHubError::BindingMismatch);
        }
        Ok(schedule)
    }

    async fn original(
        &self,
        store: &DeliverySyncStore,
        credential: &str,
        id: &str,
    ) -> Result<(Attempt, Value), GitHubError> {
        let config = &self.observer.config;
        self.current_mapping(store, credential).await?;
        let view = store
            .inspect_integration(
                &config.tenant_id,
                &config.project_id,
                credential,
                &config.work_id,
                id,
            )
            .await
            .map_err(domain_error)?;
        if view["connector_id"] != config.connector_id {
            return Err(GitHubError::BindingMismatch);
        }
        let attempt = Attempt {
            version: 1,
            config_digest: self.observer.config_digest.clone(),
            candidate: serde_json::from_value(view["candidate"].clone())
                .map_err(|_| GitHubError::InvalidResponse)?,
            request: serde_json::from_value(view["integration_request"].clone())
                .map_err(|_| GitHubError::InvalidResponse)?,
            read_set: serde_json::from_value(view["original_read_set"].clone())
                .map_err(|_| GitHubError::InvalidResponse)?,
            connector_version: view["connector_version"]
                .as_str()
                .ok_or(GitHubError::InvalidResponse)?
                .into(),
            eligibility_digest: view["eligibility_digest"]
                .as_str()
                .ok_or(GitHubError::InvalidResponse)?
                .into(),
        };
        self.validate(&attempt)?;
        if attempt.request.request_id.as_str() != id {
            return Err(GitHubError::InvalidResponse);
        }
        Ok((attempt, view))
    }

    /// Obtain the first dispatch's sealed capability from the actual store.
    /// Replays can only observe; a public receipt is never an execution input.
    pub async fn execute(
        &self,
        store: &DeliverySyncStore,
        credential: &str,
        request: DispatchDeliveryIntegration,
    ) -> Result<GitHubIntegrationSnapshot, GitHubError> {
        let (original, _) = self
            .original(store, credential, &request.integration_id)
            .await?;
        if original.request.operation != IntegrationOperation::FastForward
            || request.read_set.work_id != self.observer.config.work_id
            || request.read_set.workstream_id.to_string() != self.observer.config.workstream_id
        {
            return Err(GitHubError::BindingMismatch);
        }
        let inspection = format!("dispatch:{}", digest(request.request_id.as_bytes()));
        let dispatch = store
            .dispatch_integration(
                &self.observer.config.tenant_id,
                &self.observer.config.project_id,
                credential,
                request,
            )
            .await
            .map_err(domain_error)?;
        match dispatch.permit {
            Some(permit) => {
                self.execute_permit(store, credential, permit, &inspection)
                    .await
            }
            None => self.query_attempt(&original, &inspection).await,
        }
    }

    /// Consumes the unique non-deserializable capability. Persist the marker
    /// before preflight/command so a cancelled future cannot initiate a retry.
    pub async fn execute_permit(
        &self,
        store: &DeliverySyncStore,
        credential: &str,
        permit: DeliveryIntegrationPermit,
        inspection_id: &str,
    ) -> Result<GitHubIntegrationSnapshot, GitHubError> {
        let attempt = Attempt {
            version: 1,
            config_digest: self.observer.config_digest.clone(),
            candidate: permit.candidate().clone(),
            request: permit.request().clone(),
            read_set: permit.read_set().clone(),
            connector_version: permit.connector_version().into(),
            eligibility_digest: permit.eligibility_digest().into(),
        };
        self.validate(&attempt)?;
        if permit.connector_id() != self.observer.config.connector_id || !text(inspection_id, 128) {
            return Err(GitHubError::BindingMismatch);
        }
        let current = self.current_mapping(store, credential).await?;
        if current["connector"]["connector_version"] != attempt.connector_version {
            return Err(GitHubError::PreconditionsChanged);
        }
        self.directories()?;
        let (marker, result) = self.paths(attempt.request.request_id.as_str());
        let encoded = bytes(&attempt)?;
        if publish_once(&marker, &encoded)? {
            let config = &self.observer.config;
            let launch = LaunchGuard::new();
            let deadline = Instant::now() + Duration::from_millis(config.inspection_timeout_ms);
            let operation = async {
                self.preflight(&attempt).await?;
                store
                    .recheck_integration(&config.tenant_id, &config.project_id, credential, &permit)
                    .await
                    .map_err(domain_error)?;
                self.receive_once(&attempt, launch.0.clone(), deadline)
                    .await
            };
            let outcome = match tokio::time::timeout_at(deadline.into(), operation).await {
                Ok(Ok(outcome)) => outcome,
                Ok(Err(diagnostic)) => launch.failed(diagnostic),
                Err(_) => launch.failed(GitHubError::TimedOut),
            };
            publish(
                &result,
                &bytes(&CommandResult {
                    request_digest: digest(&encoded),
                    outcome,
                })?,
            )?;
        }
        self.query_attempt(&attempt, inspection_id).await
    }

    fn directories(&self) -> Result<(), GitHubError> {
        let path = &self.observer.config.report_directory;
        if fs::canonicalize(path).map_err(|_| GitHubError::ReportUnavailable)? != *path
            || !fs::symlink_metadata(path)
                .map_err(|_| GitHubError::ReportUnavailable)?
                .is_dir()
        {
            return Err(GitHubError::ReportUnavailable);
        }
        Ok(())
    }

    async fn preflight(&self, attempt: &Attempt) -> Result<(), GitHubError> {
        if attempt.request.operation != IntegrationOperation::FastForward {
            return Err(GitHubError::UnsupportedGuarantee);
        }
        self.observer
            .integration_preflight(&attempt.candidate)
            .await?;
        let config = &self.observer.config;
        let (url, _) = github_receive::urls(config)?;
        let reference = attempt.candidate.binding.target.reference.clone().unwrap();
        let TargetPrecondition::Exact(old) = &attempt.candidate.binding.target.precondition else {
            return Err(GitHubError::UnsupportedGuarantee);
        };
        let old = old.value.clone();
        let transport = self.receive.clone();
        let timeout = Duration::from_millis(config.request_timeout_ms);
        let limit = config.max_response_bytes;
        let response = tokio::task::spawn_blocking(move || {
            transport.request(GitHubReceiveMethod::Advertise, &url, &[], timeout, limit)
        })
        .await
        .map_err(|_| GitHubError::ProviderUnavailable)??;
        check_response(
            &response,
            "application/x-git-receive-pack-advertisement",
            limit,
        )?;
        github_receive::advertisement(&response.body, &reference, &old)
    }

    async fn receive_once(
        &self,
        attempt: &Attempt,
        launch: Arc<AtomicU8>,
        deadline: Instant,
    ) -> Result<GitHubAttemptOutcome, GitHubError> {
        let config = &self.observer.config;
        let (_, url) = github_receive::urls(config)?;
        let payload = github_receive::payload(&attempt.candidate)?;
        let reference = attempt.candidate.binding.target.reference.clone().unwrap();
        let transport = self.receive.clone();
        let timeout = Duration::from_millis(config.request_timeout_ms);
        let limit = config.max_response_bytes;
        let gate = tokio::time::timeout_at(deadline.into(), self.gate.clone().acquire_owned())
            .await
            .map_err(|_| GitHubError::TimedOut)?
            .map_err(|_| GitHubError::ProviderUnavailable)?;
        tokio::task::spawn_blocking(move || {
            let _gate = gate; // A dropped future cannot multiply blocking jobs.
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or(GitHubError::TimedOut)?;
            if launch
                .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                return Err(GitHubError::TimedOut);
            }
            let response = transport.request(
                GitHubReceiveMethod::Apply,
                &url,
                &payload,
                remaining.min(timeout),
                limit,
            )?;
            check_response(&response, "application/x-git-receive-pack-result", limit)?;
            Ok(match github_receive::status(&response.body, &reference)? {
                PushStatus::Accepted => GitHubAttemptOutcome::Accepted,
                PushStatus::Rejected => GitHubAttemptOutcome::Rejected,
            })
        })
        .await
        .map_err(|_| GitHubError::ProviderUnavailable)?
    }

    /// Read-only recovery from the actual original intent, including historical
    /// candidates. It never asks the store for a new lease or executable permit.
    pub async fn query(
        &self,
        store: &DeliverySyncStore,
        credential: &str,
        integration_id: &str,
        inspection_id: &str,
    ) -> Result<GitHubIntegrationSnapshot, GitHubError> {
        let (attempt, view) = self.original(store, credential, integration_id).await?;
        if view["dispatched_at"].is_null() {
            return Err(GitHubError::PreconditionsChanged);
        }
        self.query_attempt(&attempt, inspection_id).await
    }

    async fn query_attempt(
        &self,
        attempt: &Attempt,
        inspection_id: &str,
    ) -> Result<GitHubIntegrationSnapshot, GitHubError> {
        if !text(inspection_id, 128) {
            return Err(GitHubError::BindingMismatch);
        }
        self.directories()?;
        let request_digest = digest(&bytes(attempt)?);
        let index = self.report_index(attempt, inspection_id);
        if let Some(snapshot) = self.cached(&index, attempt, inspection_id, &request_digest)? {
            return Ok(snapshot);
        }
        let report = self.probe_attempt(attempt, inspection_id).await?;
        self.publish_probe(attempt, report, inspection_id)
    }

    fn report_index(&self, attempt: &Attempt, inspection_id: &str) -> PathBuf {
        let key = digest(
            &serde_json::to_vec(&(
                attempt.request.request_id.as_str(),
                inspection_id,
                &self.observer.config.adapter_id,
            ))
            .unwrap(),
        );
        self.observer
            .config
            .report_directory
            .join(format!("github-integration-inspection-{key}.json"))
    }

    /// Fresh bounded reads without publishing an observation or effect marker.
    async fn probe_attempt(
        &self,
        attempt: &Attempt,
        inspection_id: &str,
    ) -> Result<GitHubIntegrationReport, GitHubError> {
        self.directories()?;
        let encoded = bytes(attempt)?;
        let request_digest = digest(&encoded);
        let (marker, result) = self.paths(attempt.request.request_id.as_str());
        let recorded = read_file(&marker)?;
        if recorded.as_ref().is_some_and(|b| *b != encoded) {
            return Err(GitHubError::ReportConflict);
        }
        let outcome = read_file(&result)?
            .map(|encoded| {
                let result: CommandResult =
                    serde_json::from_slice(&encoded).map_err(|_| GitHubError::ReportUnavailable)?;
                if recorded.is_none() || result.request_digest != request_digest {
                    return Err(GitHubError::ReportConflict);
                }
                Ok(result.outcome)
            })
            .transpose()?;
        let observation = tokio::time::timeout(
            Duration::from_millis(self.observer.config.inspection_timeout_ms),
            self.observer
                .probe_original(&attempt.candidate, inspection_id),
        )
        .await
        .map_err(|_| GitHubError::TimedOut)
        .and_then(|r| r);
        let (observed, diagnostic) = match observation {
            Ok(report) => (Some(report), None),
            Err(error) => (None, Some(error)),
        };
        let terminal = if observed
            .as_ref()
            .is_some_and(|r| r.integration_outcome == IntegrationOutcome::Applied)
        {
            IntegrationOutcome::Applied
        } else if observed.as_ref().is_some_and(|r| r.target_stable)
            && matches!(
                outcome,
                Some(GitHubAttemptOutcome::NotStarted { .. } | GitHubAttemptOutcome::Rejected)
            )
        {
            IntegrationOutcome::Rejected
        } else {
            // Missing/unchanged target, a failed process or an absent report do
            // not establish no effect. Keep the durable target guard unresolved.
            IntegrationOutcome::Unknown
        };
        Ok(GitHubIntegrationReport {
            version: 1,
            config_digest: self.observer.config_digest.clone(),
            candidate_digest: attempt.candidate.binding.digest().unwrap(),
            request_digest: request_digest.clone(),
            integration_id: attempt.request.request_id.as_str().into(),
            inspection_id: inspection_id.into(),
            attempt_recorded: recorded.is_some(),
            command_outcome: outcome,
            observed_at_unix_ms: observed
                .as_ref()
                .map(|r| r.observed_at_unix_ms)
                .unwrap_or_else(now_ms),
            observation: observed,
            diagnostic,
            outcome: terminal,
        })
    }

    fn publish_probe(
        &self,
        attempt: &Attempt,
        mut report: GitHubIntegrationReport,
        inspection_id: &str,
    ) -> Result<GitHubIntegrationSnapshot, GitHubError> {
        if !text(inspection_id, 128) {
            return Err(GitHubError::BindingMismatch);
        }
        report.inspection_id = inspection_id.into();
        if let Some(observation) = &mut report.observation {
            observation.inspection_id = inspection_id.into();
        }
        let request_digest = digest(&bytes(attempt)?);
        let index = self.report_index(attempt, inspection_id);
        let encoded = bytes(&report)?;
        let report_sha256 = digest(&encoded);
        publish(
            &self
                .observer
                .config
                .report_directory
                .join(format!("github-integration-report-{report_sha256}.json")),
            &encoded,
        )?;
        match publish(
            &index,
            &bytes(&ReportIndex {
                request_digest: request_digest.clone(),
                report_sha256,
            })?,
        ) {
            Ok(()) | Err(super::LocalGitError::ReportConflict) => self
                .cached(&index, attempt, inspection_id, &request_digest)?
                .ok_or(GitHubError::ReportUnavailable),
            Err(error) => Err(error.into()),
        }
    }

    pub fn report_bytes(&self, sha256: &str) -> Result<Vec<u8>, GitHubError> {
        if sha256.len() != 64
            || !sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(GitHubError::ReportUnavailable);
        }
        let encoded = read_file(
            &self
                .observer
                .config
                .report_directory
                .join(format!("github-integration-report-{sha256}.json")),
        )?
        .ok_or(GitHubError::ReportUnavailable)?;
        if digest(&encoded) != sha256 {
            return Err(GitHubError::ReportConflict);
        }
        Ok(encoded)
    }

    fn cached(
        &self,
        path: &std::path::Path,
        attempt: &Attempt,
        inspection_id: &str,
        request_digest: &str,
    ) -> Result<Option<GitHubIntegrationSnapshot>, GitHubError> {
        let Some(encoded) = read_file(path)? else {
            return Ok(None);
        };
        let index: ReportIndex =
            serde_json::from_slice(&encoded).map_err(|_| GitHubError::ReportUnavailable)?;
        if index.request_digest != request_digest {
            return Err(GitHubError::ReportConflict);
        }
        let encoded = self.report_bytes(&index.report_sha256)?;
        let report: GitHubIntegrationReport =
            serde_json::from_slice(&encoded).map_err(|_| GitHubError::ReportUnavailable)?;
        if report.version != 1
            || report.request_digest != request_digest
            || report.config_digest != self.observer.config_digest
            || report.candidate_digest != attempt.candidate.binding.digest().unwrap()
            || report.integration_id != attempt.request.request_id.as_str()
            || report.inspection_id != inspection_id
        {
            return Err(GitHubError::ReportConflict);
        }
        let artifact = ArtifactEntry {
            artifact_id: format!("github-integration-{}", index.report_sha256),
            sha256: index.report_sha256.clone(),
            byte_length: encoded.len().to_string(),
            locator: format!(
                "awr-github-integration:{}:{}",
                self.observer.config.adapter_id, index.report_sha256
            ),
        };
        let observation = IntegrationObservation {
            binding: attempt.candidate.binding.clone(),
            request_id: Some(attempt.request.request_id.clone()),
            external_reference: format!(
                "github-target:{}:{}",
                self.observer.config.repository_id, self.observer.config.target_branch
            ),
            outcome: report.outcome.clone(),
            result_revision: report
                .observation
                .as_ref()
                .and_then(|r| r.target_revision.clone()),
            contains_manifest_digest: (report.outcome == IntegrationOutcome::Applied)
                .then(|| attempt.candidate.binding.manifest_digest.clone()),
            provenance: FactProvenance {
                source: FactSource::AdapterObservation,
                reference: artifact.locator.clone(),
                observed_at_unix_ms: Some(report.observed_at_unix_ms),
                recorded_at_unix_ms: report.observed_at_unix_ms,
            },
        };
        let record = DeliveryEnvelope {
            protocol: DELIVERY_PROTOCOL.into(),
            protocol_version: DELIVERY_PROTOCOL_VERSION,
            record: DeliveryRecord::IntegrationObservation(observation),
        };
        record.validate().map_err(|_| GitHubError::ReportConflict)?;
        Ok(Some(GitHubIntegrationSnapshot {
            report,
            report_artifact: artifact,
            records: vec![record],
        }))
    }

    /// Reserve and ingest through the existing neutral inbox, then confirm only
    /// this original effect. Task acceptance and source finalization stay separate.
    pub async fn reconcile(
        &self,
        store: &DeliverySyncStore,
        credential: &str,
        request: GitHubIntegrationPollRequest,
    ) -> Result<Value, GitHubError> {
        if !text(&request.request_id, 80)
            || request.read_set.work_id != self.observer.config.work_id
            || request.read_set.workstream_id.to_string() != self.observer.config.workstream_id
        {
            return Err(GitHubError::BindingMismatch);
        }
        let (original, _) = self
            .original(store, credential, &request.integration_id)
            .await?;
        let config = &self.observer.config;
        let reserved = store
            .reserve_integration_inspection(
                &config.tenant_id,
                &config.project_id,
                credential,
                &request.integration_id,
                ReserveDeliveryInspection {
                    request_id: format!("{}:reserve", request.request_id),
                    read_set: request.read_set.clone(),
                    connector_id: config.connector_id.clone(),
                    connector_version: request.connector_version.clone(),
                    candidate_digest: original.candidate.binding.digest().unwrap(),
                    lease_seconds: 120,
                },
            )
            .await
            .map_err(domain_error)?;
        self.reconcile_reserved(store, credential, request, original, &reserved)
            .await
    }

    async fn reconcile_reserved(
        &self,
        store: &DeliverySyncStore,
        credential: &str,
        request: GitHubIntegrationPollRequest,
        original: Attempt,
        reserved: &Value,
    ) -> Result<Value, GitHubError> {
        let config = &self.observer.config;
        let inspection = reserved["data"]["inspection_id"]
            .as_str()
            .ok_or(GitHubError::InvalidResponse)?;
        let snapshot = self.query_attempt(&original, inspection).await?;
        let ingested = store
            .ingest_facts(
                &config.tenant_id,
                &config.project_id,
                credential,
                IngestDeliveryFacts {
                    request_id: format!("{}:ingest", request.request_id),
                    read_set: request.read_set.clone(),
                    connector_id: config.connector_id.clone(),
                    inspection_id: inspection.into(),
                    event_id: format!("{}:observation", request.request_id),
                    records: snapshot.records,
                },
            )
            .await
            .map_err(domain_error)?;
        let fact = ingested["data"]["observation_receipt"]["fact_ids"][0]
            .as_str()
            .ok_or(GitHubError::InvalidResponse)?;
        let id = request.integration_id.clone();
        let confirmed = store
            .confirm_integration(
                &config.tenant_id,
                &config.project_id,
                credential,
                ConfirmDeliveryIntegration {
                    request_id: format!("{}:confirm", request.request_id),
                    read_set: request.read_set,
                    integration_id: request.integration_id,
                    fact_id: fact.into(),
                },
            )
            .await;
        let confirmed = match confirmed {
            Ok(receipt) => receipt,
            Err(awr_team_pg::PgError::RecoveryBlocked) => {
                let view = store
                    .inspect_integration(
                        &config.tenant_id,
                        &config.project_id,
                        credential,
                        &config.work_id,
                        &id,
                    )
                    .await
                    .map_err(domain_error)?;
                if !matches!(view["state"].as_str(), Some("confirmed" | "rejected")) {
                    return Err(GitHubError::PreconditionsChanged);
                }
                view
            }
            Err(error) => return Err(domain_error(error)),
        };
        Ok(
            json!({"observation":ingested,"integration":confirmed,"unchanged":false,
                "read_only":false,"execution_authorized":false,"acceptance_ready":false,"source_synchronized":false}),
        )
    }
}
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn check_response(
    response: &super::github_receive::GitHubReceiveResponse,
    expected: &str,
    limit: usize,
) -> Result<(), GitHubError> {
    if response.body.len() > limit {
        return Err(GitHubError::OutputLimit);
    }
    if response.status != 200 {
        return Err(match response.status {
            401 | 403 => GitHubError::ProviderAuthorizationUnavailable,
            429 => GitHubError::RateLimited,
            _ => GitHubError::ProviderUnavailable,
        });
    }
    if response
        .content_type
        .as_deref()
        .and_then(|s| s.split(';').next())
        .map(str::trim)
        != Some(expected)
    {
        return Err(GitHubError::InvalidResponse);
    }
    Ok(())
}
