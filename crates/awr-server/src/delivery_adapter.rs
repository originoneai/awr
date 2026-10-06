//! Optional provider adapters. Observation is neither approval nor completion.
mod git_process;
pub mod local_git;
pub mod local_git_integration;
mod local_git_poll;

pub use local_git::{LocalGitAdapter, LocalGitConfig, LocalGitReport, LocalGitSnapshot};
pub use local_git_integration::{
    LocalGitIntegrationConfig, LocalGitIntegrationPollRequest, LocalGitIntegrationReport,
    LocalGitIntegrationSnapshot, LocalGitIntegrator,
};

use awr_team_pg::{
    DeliveryReadSet, DeliverySyncStore, IngestDeliveryFacts, PgError, ReserveDeliveryInspection,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

const REPORT_LIMIT: usize = 65536;

/// Finite diagnostics deliberately exclude provider stderr, repository contents and credentials.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalGitError {
    InvalidConfiguration,
    BindingMismatch,
    RepositoryUnavailable,
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
    InvalidStoreResponse,
}

fn domain_error(error: PgError) -> LocalGitError {
    match error {
        PgError::Forbidden
        | PgError::ProjectNotAvailable
        | PgError::Workstream(awr_core::WorkstreamError::AccessDenied) => {
            LocalGitError::AuthorizationUnavailable
        }
        PgError::PreconditionsChanged
        | PgError::EpochChanged
        | PgError::SourceDivergence
        | PgError::InactiveCandidate
        | PgError::StaleFence
        | PgError::LeaseExpired
        | PgError::SchemaIncompatible(_)
        | PgError::Workstream(_) => LocalGitError::PreconditionsChanged,
        PgError::ResourceConflict | PgError::ClaimHeld => LocalGitError::Contention,
        PgError::IdempotencyConflict => LocalGitError::IdempotencyConflict,
        PgError::BindingInvalid | PgError::Protocol(_) => LocalGitError::DomainRejected,
        _ => LocalGitError::StoreUnavailable,
    }
}

pub(super) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

pub(super) fn read_file(path: &Path) -> Result<Option<Vec<u8>>, LocalGitError> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(LocalGitError::ReportUnavailable),
    };
    if !metadata.is_file() || metadata.len() > REPORT_LIMIT as u64 {
        return Err(LocalGitError::ReportUnavailable);
    }
    let file = options
        .open(path)
        .map_err(|_| LocalGitError::ReportUnavailable)?;
    if !file
        .metadata()
        .map_err(|_| LocalGitError::ReportUnavailable)?
        .is_file()
    {
        return Err(LocalGitError::ReportUnavailable);
    }
    let mut bytes = Vec::new();
    file.take(REPORT_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| LocalGitError::ReportUnavailable)?;
    if bytes.len() > REPORT_LIMIT {
        return Err(LocalGitError::OutputLimit);
    }
    Ok(Some(bytes))
}

/// Publish a complete immutable file without replacing a concurrent winner.
pub(super) fn publish(path: &Path, bytes: &[u8]) -> Result<(), LocalGitError> {
    publish_once(path, bytes).map(|_| ())
}

/// Only the creator may initiate an effect. An identical existing file is a replay.
pub(super) fn publish_once(path: &Path, bytes: &[u8]) -> Result<bool, LocalGitError> {
    if bytes.len() > REPORT_LIMIT {
        return Err(LocalGitError::OutputLimit);
    }
    let temporary = path.with_extension(format!("{}.tmp", awr_core::Id::new()));
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|_| LocalGitError::ReportUnavailable)?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| LocalGitError::ReportUnavailable)?;
        match fs::hard_link(&temporary, path) {
            Ok(()) => {
                #[cfg(unix)]
                fs::File::open(path.parent().ok_or(LocalGitError::ReportUnavailable)?)
                    .and_then(|directory| directory.sync_all())
                    .map_err(|_| LocalGitError::ReportUnavailable)?;
                Ok(true)
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if read_file(path)?.as_deref() == Some(bytes) {
                    Ok(false)
                } else {
                    Err(LocalGitError::ReportConflict)
                }
            }
            Err(_) => Err(LocalGitError::ReportUnavailable),
        }
    })();
    let _ = fs::remove_file(&temporary);
    result
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalGitPollRequest {
    /// Stable across a retry after a missing receipt; new IDs request new observations.
    pub request_id: String,
    pub read_set: DeliveryReadSet,
    pub connector_version: String,
}

impl LocalGitAdapter {
    /// Library entry point for a configured service principal, not a new public RPC.
    /// Reserve against the real selected candidate before reading the repository.
    /// PG independently enforces current identity, connector provenance and generations.
    pub async fn reconcile(
        &self,
        store: &DeliverySyncStore,
        credential: &str,
        request: LocalGitPollRequest,
    ) -> Result<Value, LocalGitError> {
        if !text(&request.request_id, 80)
            || request.read_set.work_id != self.config.work_id
            || request.read_set.workstream_id.to_string() != self.config.workstream_id
        {
            return Err(LocalGitError::BindingMismatch);
        }
        let view = store
            .inspect(
                &self.config.tenant_id,
                &self.config.project_id,
                credential,
                &self.config.work_id,
            )
            .await
            .map_err(domain_error)?;
        if view["selected_current"] != true {
            return Err(LocalGitError::PreconditionsChanged);
        }
        let candidate = serde_json::from_value(view["candidate"].clone())
            .map_err(|_| LocalGitError::InvalidStoreResponse)?;
        self.validate_candidate(&candidate)?;
        let reserved = store
            .reserve_inspection(
                &self.config.tenant_id,
                &self.config.project_id,
                credential,
                ReserveDeliveryInspection {
                    request_id: format!("{}:reserve", request.request_id),
                    read_set: request.read_set.clone(),
                    connector_id: self.config.connector_id.clone(),
                    connector_version: request.connector_version,
                    candidate_digest: candidate
                        .binding
                        .digest()
                        .map_err(|_| LocalGitError::BindingMismatch)?,
                    lease_seconds: 120,
                },
            )
            .await
            .map_err(domain_error)?;
        let inspection = reserved["data"]["inspection_id"]
            .as_str()
            .ok_or(LocalGitError::InvalidStoreResponse)?;
        let snapshot = self.inspect(&candidate, inspection).await?;
        store
            .ingest_facts(
                &self.config.tenant_id,
                &self.config.project_id,
                credential,
                IngestDeliveryFacts {
                    request_id: format!("{}:ingest", request.request_id),
                    read_set: request.read_set,
                    connector_id: self.config.connector_id.clone(),
                    inspection_id: inspection.to_owned(),
                    event_id: format!("{}:observation", request.request_id),
                    records: snapshot.records,
                },
            )
            .await
            .map_err(domain_error)
    }
}

#[cfg(test)]
mod publication_tests {
    use super::*;

    #[test]
    fn publication_coordination_is_contention_not_database_failure() {
        assert_eq!(
            domain_error(PgError::ResourceConflict),
            LocalGitError::Contention
        );
        assert_eq!(domain_error(PgError::ClaimHeld), LocalGitError::Contention);
        assert_eq!(
            domain_error(PgError::PreconditionsChanged),
            LocalGitError::PreconditionsChanged
        );
        assert_eq!(
            domain_error(PgError::Forbidden),
            LocalGitError::AuthorizationUnavailable
        );
    }

    #[test]
    fn identical_concurrent_marker_publication_has_exactly_one_creator() {
        let directory = std::env::temp_dir().join(format!("awr-attempt-{}", awr_core::Id::new()));
        fs::create_dir(&directory).unwrap();
        let marker = directory.join("attempt.json");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let mut callers = Vec::new();
        for _ in 0..8 {
            let barrier = barrier.clone();
            let marker = marker.clone();
            callers.push(std::thread::spawn(move || {
                barrier.wait();
                publish_once(&marker, b"immutable complete attempt").unwrap()
            }));
        }
        let creators = callers
            .into_iter()
            .map(|t| t.join().unwrap())
            .filter(|created| *created)
            .count();
        assert_eq!(creators, 1);
        assert!(!publish_once(&marker, b"immutable complete attempt").unwrap());
        assert_eq!(
            publish_once(&marker, b"different attempt"),
            Err(LocalGitError::ReportConflict)
        );
        publish(&marker, b"immutable complete attempt").unwrap();
        assert_eq!(
            read_file(&marker).unwrap().unwrap(),
            b"immutable complete attempt"
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
