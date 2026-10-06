//! Opt-in fast-forward effects. Recovery observes the original request only.
use super::{
    LocalGitAdapter, LocalGitConfig, LocalGitError, LocalGitReport, REPORT_LIMIT, digest,
    domain_error, publish, publish_once, read_file, text,
};
use awr_team::delivery::*;
use awr_team_pg::{
    ConfirmDeliveryIntegration, DeliveryIntegrationPermit, DeliveryReadSet, DeliverySyncStore,
    DispatchDeliveryIntegration, IngestDeliveryFacts, ReserveDeliveryInspection,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{fs, path::PathBuf, time::Duration};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalGitIntegrationConfig {
    pub enabled: bool,
    pub repository: LocalGitConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalGitIntegrationPollRequest {
    /// Retry this ID for the same immutable observation; use a new ID to requery.
    pub request_id: String,
    pub integration_id: String,
    /// Current admission, distinct from the original candidate's historical set.
    pub read_set: DeliveryReadSet,
    pub connector_version: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum LocalGitAttemptOutcome {
    NotStarted { diagnostic: LocalGitError },
    Exited { code: i32 },
    Uncertain { diagnostic: LocalGitError },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalGitIntegrationReport {
    pub version: u32,
    pub config_digest: String,
    pub candidate_digest: String,
    pub request_digest: String,
    pub integration_id: String,
    pub inspection_id: String,
    pub attempt_recorded: bool,
    pub command_outcome: Option<LocalGitAttemptOutcome>,
    pub observation: Option<LocalGitReport>,
    pub diagnostic: Option<LocalGitError>,
    pub outcome: IntegrationOutcome,
    pub observed_at_unix_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalGitIntegrationSnapshot {
    pub report: LocalGitIntegrationReport,
    pub report_artifact: ArtifactEntry,
    pub records: Vec<DeliveryEnvelope>,
}

#[derive(Serialize, Deserialize)]
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
    outcome: LocalGitAttemptOutcome,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportIndex {
    request_digest: String,
    report_sha256: String,
}

fn bytes(value: &impl Serialize) -> Result<Vec<u8>, LocalGitError> {
    let encoded = serde_json::to_vec(value).map_err(|_| LocalGitError::ReportUnavailable)?;
    if encoded.len() > REPORT_LIMIT {
        return Err(LocalGitError::OutputLimit);
    }
    Ok(encoded)
}

pub struct LocalGitIntegrator {
    observer: LocalGitAdapter,
}

impl LocalGitIntegrator {
    /// Opt-in mechanical support alone grants no AWR authority.
    pub async fn open(config: LocalGitIntegrationConfig) -> Result<Self, LocalGitError> {
        if !config.enabled {
            return Err(LocalGitError::InvalidConfiguration);
        }
        Ok(Self {
            observer: LocalGitAdapter::open(config.repository).await?,
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
                .join(format!("integration-{key}-attempt.json")),
            config
                .report_directory
                .join(format!("integration-{key}-result.json")),
        )
    }

    fn validate(&self, attempt: &Attempt) -> Result<(), LocalGitError> {
        self.observer.validate_candidate(&attempt.candidate)?;
        DeliveryRecord::IntegrationRequest(attempt.request.clone())
            .validate_against(&attempt.candidate.binding)
            .map_err(|_| LocalGitError::BindingMismatch)?;
        let config = &self.observer.config;
        if attempt.version != 1
            || attempt.config_digest != self.observer.config_digest
            || attempt.read_set.work_id != config.work_id
            || attempt.read_set.workstream_id.to_string() != config.workstream_id
            || attempt.read_set.contract_hash != attempt.candidate.binding.contract_hash
        {
            return Err(LocalGitError::BindingMismatch);
        }
        Ok(())
    }

    async fn original(
        &self,
        store: &DeliverySyncStore,
        credential: &str,
        id: &str,
    ) -> Result<(Attempt, Value), LocalGitError> {
        let config = &self.observer.config;
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
            return Err(LocalGitError::BindingMismatch);
        }
        let attempt = Attempt {
            version: 1,
            config_digest: self.observer.config_digest.clone(),
            candidate: serde_json::from_value(view["candidate"].clone())
                .map_err(|_| LocalGitError::InvalidStoreResponse)?,
            request: serde_json::from_value(view["integration_request"].clone())
                .map_err(|_| LocalGitError::InvalidStoreResponse)?,
            read_set: serde_json::from_value(view["original_read_set"].clone())
                .map_err(|_| LocalGitError::InvalidStoreResponse)?,
            connector_version: view["connector_version"]
                .as_str()
                .ok_or(LocalGitError::InvalidStoreResponse)?
                .into(),
            eligibility_digest: view["eligibility_digest"]
                .as_str()
                .ok_or(LocalGitError::InvalidStoreResponse)?
                .into(),
        };
        self.validate(&attempt)?;
        if attempt.request.request_id.as_str() != id {
            return Err(LocalGitError::InvalidStoreResponse);
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
    ) -> Result<LocalGitIntegrationSnapshot, LocalGitError> {
        let (original, _) = self
            .original(store, credential, &request.integration_id)
            .await?;
        if original.request.operation != IntegrationOperation::FastForward
            || request.read_set.work_id != self.observer.config.work_id
            || request.read_set.workstream_id.to_string() != self.observer.config.workstream_id
        {
            return Err(LocalGitError::BindingMismatch);
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
    ) -> Result<LocalGitIntegrationSnapshot, LocalGitError> {
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
            return Err(LocalGitError::BindingMismatch);
        }
        self.directories()?;
        let (marker, result) = self.paths(attempt.request.request_id.as_str());
        let encoded = bytes(&attempt)?;
        if publish_once(&marker, &encoded)? {
            let config = &self.observer.config;
            let admission = async {
                self.preflight(&attempt).await?;
                store
                    .recheck_integration(&config.tenant_id, &config.project_id, credential, &permit)
                    .await
                    .map_err(domain_error)
            }
            .await;
            let outcome = match admission {
                Err(diagnostic) => LocalGitAttemptOutcome::NotStarted { diagnostic },
                Ok(()) => {
                    let source = attempt
                        .candidate
                        .binding
                        .source_revision
                        .as_ref()
                        .ok_or(LocalGitError::BindingMismatch)?;
                    let old = match &attempt.candidate.binding.target.precondition {
                        TargetPrecondition::Exact(revision) => revision.value.clone(),
                        TargetPrecondition::Missing => "0".repeat(source.value.len()),
                    };
                    // Fixed CAS of this named ref only. No push, helpers, working
                    // tree edits, merge/squash/rebase, caller shell or hooks.
                    match self
                        .observer
                        .git
                        .run(
                            &[
                                "update-ref",
                                "--no-deref",
                                &config.target_reference,
                                &source.value,
                                &old,
                            ],
                            4096,
                        )
                        .await
                    {
                        Ok(output) => LocalGitAttemptOutcome::Exited { code: output.code },
                        Err(diagnostic) => LocalGitAttemptOutcome::Uncertain { diagnostic },
                    }
                }
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

    fn directories(&self) -> Result<(), LocalGitError> {
        for path in [
            &self.observer.config.repository,
            &self.observer.config.report_directory,
        ] {
            if fs::canonicalize(path).map_err(|_| LocalGitError::RepositoryUnavailable)? != *path
                || !fs::symlink_metadata(path)
                    .map_err(|_| LocalGitError::RepositoryUnavailable)?
                    .is_dir()
            {
                return Err(LocalGitError::RepositoryUnavailable);
            }
        }
        Ok(())
    }

    async fn preflight(&self, attempt: &Attempt) -> Result<(), LocalGitError> {
        if attempt.request.operation != IntegrationOperation::FastForward
            || attempt
                .candidate
                .binding
                .source_revision
                .as_ref()
                .is_none_or(|s| s.format != self.observer.format)
            || matches!(&attempt.candidate.binding.target.precondition,
                TargetPrecondition::Exact(r) if r.format != self.observer.format)
        {
            return Err(LocalGitError::BindingMismatch);
        }
        // The configured total budget must cover both source and target bytes;
        // otherwise an effect could be launched that its recovery cannot verify.
        let total = attempt
            .candidate
            .manifest
            .entries
            .iter()
            .try_fold(0usize, |sum, entry| {
                entry
                    .byte_length
                    .parse::<usize>()
                    .ok()
                    .and_then(|size| sum.checked_add(size))
            })
            .ok_or(LocalGitError::OutputLimit)?;
        if total > self.observer.config.max_total_blob_bytes / 2 {
            return Err(LocalGitError::OutputLimit);
        }
        let report = tokio::time::timeout(
            Duration::from_millis(self.observer.config.inspection_timeout_ms),
            self.observer.observe(
                &attempt.candidate,
                "integration-preflight",
                attempt.candidate.binding.digest().unwrap(),
            ),
        )
        .await
        .map_err(|_| LocalGitError::TimedOut)??;
        if report.manifest_outcome != VerificationOutcome::Passed
            || !report.target_stable
            || !report.target_precondition_matches
        {
            return Err(LocalGitError::PreconditionsChanged);
        }
        if let Some(target) = report.target_revision {
            let source = attempt.candidate.binding.source_revision.as_ref().unwrap();
            if self
                .observer
                .git
                .run(
                    &["merge-base", "--is-ancestor", &target.value, &source.value],
                    4096,
                )
                .await?
                .code
                != 0
            {
                return Err(LocalGitError::PreconditionsChanged);
            }
        }
        Ok(())
    }

    /// Read-only recovery from the actual original intent, including historical
    /// candidates. It never asks the store for a new lease or executable permit.
    pub async fn query(
        &self,
        store: &DeliverySyncStore,
        credential: &str,
        integration_id: &str,
        inspection_id: &str,
    ) -> Result<LocalGitIntegrationSnapshot, LocalGitError> {
        let (attempt, view) = self.original(store, credential, integration_id).await?;
        if view["dispatched_at"].is_null() {
            return Err(LocalGitError::PreconditionsChanged);
        }
        self.query_attempt(&attempt, inspection_id).await
    }

    async fn query_attempt(
        &self,
        attempt: &Attempt,
        inspection_id: &str,
    ) -> Result<LocalGitIntegrationSnapshot, LocalGitError> {
        if !text(inspection_id, 128) {
            return Err(LocalGitError::BindingMismatch);
        }
        self.directories()?;
        let encoded = bytes(attempt)?;
        let request_digest = digest(&encoded);
        let key = digest(
            &serde_json::to_vec(&(
                attempt.request.request_id.as_str(),
                inspection_id,
                &self.observer.config.adapter_id,
            ))
            .unwrap(),
        );
        let index = self
            .observer
            .config
            .report_directory
            .join(format!("integration-inspection-{key}.json"));
        if let Some(snapshot) = self.cached(&index, attempt, inspection_id, &request_digest)? {
            return Ok(snapshot);
        }
        let (marker, result) = self.paths(attempt.request.request_id.as_str());
        let recorded = read_file(&marker)?;
        if recorded.as_ref().is_some_and(|b| *b != encoded) {
            return Err(LocalGitError::ReportConflict);
        }
        let outcome = read_file(&result)?
            .map(|encoded| {
                let result: CommandResult = serde_json::from_slice(&encoded)
                    .map_err(|_| LocalGitError::ReportUnavailable)?;
                if recorded.is_none() || result.request_digest != request_digest {
                    return Err(LocalGitError::ReportConflict);
                }
                Ok(result.outcome)
            })
            .transpose()?;
        let observation = tokio::time::timeout(
            Duration::from_millis(self.observer.config.inspection_timeout_ms),
            self.observer.observe(
                &attempt.candidate,
                inspection_id,
                attempt.candidate.binding.digest().unwrap(),
            ),
        )
        .await
        .map_err(|_| LocalGitError::TimedOut)
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
            && matches!(outcome, Some(LocalGitAttemptOutcome::NotStarted { .. }))
        {
            IntegrationOutcome::Rejected
        } else {
            // Missing/unchanged target, a failed process or an absent report do
            // not establish no effect. Keep the durable target guard unresolved.
            IntegrationOutcome::Unknown
        };
        let report = LocalGitIntegrationReport {
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
        };
        let encoded = bytes(&report)?;
        let report_sha256 = digest(&encoded);
        publish(
            &self
                .observer
                .config
                .report_directory
                .join(format!("integration-report-{report_sha256}.json")),
            &encoded,
        )?;
        match publish(
            &index,
            &bytes(&ReportIndex {
                request_digest: request_digest.clone(),
                report_sha256,
            })?,
        ) {
            Ok(()) | Err(LocalGitError::ReportConflict) => self
                .cached(&index, attempt, inspection_id, &request_digest)?
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
        let encoded = read_file(
            &self
                .observer
                .config
                .report_directory
                .join(format!("integration-report-{sha256}.json")),
        )?
        .ok_or(LocalGitError::ReportUnavailable)?;
        if digest(&encoded) != sha256 {
            return Err(LocalGitError::ReportConflict);
        }
        Ok(encoded)
    }

    fn cached(
        &self,
        path: &std::path::Path,
        attempt: &Attempt,
        inspection_id: &str,
        request_digest: &str,
    ) -> Result<Option<LocalGitIntegrationSnapshot>, LocalGitError> {
        let Some(encoded) = read_file(path)? else {
            return Ok(None);
        };
        let index: ReportIndex =
            serde_json::from_slice(&encoded).map_err(|_| LocalGitError::ReportUnavailable)?;
        if index.request_digest != request_digest {
            return Err(LocalGitError::ReportConflict);
        }
        let encoded = self.report_bytes(&index.report_sha256)?;
        let report: LocalGitIntegrationReport =
            serde_json::from_slice(&encoded).map_err(|_| LocalGitError::ReportUnavailable)?;
        if report.version != 1
            || report.request_digest != request_digest
            || report.config_digest != self.observer.config_digest
            || report.candidate_digest != attempt.candidate.binding.digest().unwrap()
            || report.integration_id != attempt.request.request_id.as_str()
            || report.inspection_id != inspection_id
        {
            return Err(LocalGitError::ReportConflict);
        }
        let artifact = ArtifactEntry {
            artifact_id: format!("local-git-integration-{}", index.report_sha256),
            sha256: index.report_sha256.clone(),
            byte_length: encoded.len().to_string(),
            locator: format!(
                "awr-local-git-integration:{}:{}",
                self.observer.config.adapter_id, index.report_sha256
            ),
        };
        let observation = IntegrationObservation {
            binding: attempt.candidate.binding.clone(),
            request_id: Some(attempt.request.request_id.clone()),
            external_reference: format!("local-git-target:{}", self.observer.config.adapter_id),
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
        record
            .validate()
            .map_err(|_| LocalGitError::ReportConflict)?;
        Ok(Some(LocalGitIntegrationSnapshot {
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
        request: LocalGitIntegrationPollRequest,
    ) -> Result<Value, LocalGitError> {
        if !text(&request.request_id, 80)
            || request.read_set.work_id != self.observer.config.work_id
            || request.read_set.workstream_id.to_string() != self.observer.config.workstream_id
        {
            return Err(LocalGitError::BindingMismatch);
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
                    connector_version: request.connector_version,
                    candidate_digest: original.candidate.binding.digest().unwrap(),
                    lease_seconds: 120,
                },
            )
            .await
            .map_err(domain_error)?;
        let inspection = reserved["data"]["inspection_id"]
            .as_str()
            .ok_or(LocalGitError::InvalidStoreResponse)?;
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
            .ok_or(LocalGitError::InvalidStoreResponse)?;
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
                    return Err(LocalGitError::PreconditionsChanged);
                }
                view
            }
            Err(error) => return Err(domain_error(error)),
        };
        Ok(
            json!({"observation":ingested,"integration":confirmed,"acceptance_ready":false,"source_synchronized":false}),
        )
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
