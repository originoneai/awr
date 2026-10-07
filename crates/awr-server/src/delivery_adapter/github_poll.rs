//! Explicit observations use current authenticated connector admission and the durable inbox.
use super::{GitHubAdapter, GitHubError};
use awr_team::delivery::DeliveryCandidate;
use awr_team_pg::{
    DeliveryReadSet, DeliveryScheduleQuery, DeliverySyncStore, IngestDeliveryFacts, PgError,
    ReserveDeliveryInspection,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubPollRequest {
    /// Stable for recovery of a missing receipt; a new ID requests a new observation.
    pub request_id: String,
    pub read_set: DeliveryReadSet,
    pub connector_version: String,
}

fn error(e: PgError) -> GitHubError {
    use super::LocalGitError;
    match super::domain_error(e) {
        LocalGitError::AuthorizationUnavailable => GitHubError::AuthorizationUnavailable,
        LocalGitError::PreconditionsChanged => GitHubError::PreconditionsChanged,
        LocalGitError::IdempotencyConflict => GitHubError::IdempotencyConflict,
        LocalGitError::Contention => GitHubError::Contention,
        LocalGitError::DomainRejected => GitHubError::DomainRejected,
        _ => GitHubError::StoreUnavailable,
    }
}

impl GitHubAdapter {
    /// Trusted configured service entry, not a caller-supplied provider or HTTP endpoint.
    /// Replays still require current identity, selection and connector authority.
    pub async fn reconcile(
        &self,
        store: &DeliverySyncStore,
        credential: &str,
        request: GitHubPollRequest,
    ) -> Result<Value, GitHubError> {
        let config = &self.config;
        if !super::text(&request.request_id, 80)
            || request.read_set.work_id != config.work_id
            || request.read_set.workstream_id.to_string() != config.workstream_id
        {
            return Err(GitHubError::BindingMismatch);
        }
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
            .map_err(error)?;
        if schedule["selection_state"] != "current"
            || schedule["connector"]["connector_version"] != request.connector_version
            || schedule["read_set"] != json!(request.read_set)
        {
            return Err(GitHubError::PreconditionsChanged);
        }
        if schedule["connector"]["provider"] != "github"
            || schedule["connector"]["resource"] != config.resource
        {
            return Err(GitHubError::BindingMismatch);
        }
        let candidate: DeliveryCandidate =
            serde_json::from_value(schedule["selection"]["candidate"].clone())
                .map_err(|_| GitHubError::InvalidResponse)?;
        self.validate_candidate(&candidate)?;
        let reserved = store
            .reserve_inspection(
                &config.tenant_id,
                &config.project_id,
                credential,
                ReserveDeliveryInspection {
                    request_id: format!("{}:reserve", request.request_id),
                    read_set: request.read_set.clone(),
                    connector_id: config.connector_id.clone(),
                    connector_version: request.connector_version,
                    candidate_digest: candidate
                        .binding
                        .digest()
                        .map_err(|_| GitHubError::BindingMismatch)?,
                    lease_seconds: 120,
                },
            )
            .await
            .map_err(error)?;
        let inspection = reserved["data"]["inspection_id"]
            .as_str()
            .ok_or(GitHubError::InvalidResponse)?;
        let snapshot = self.inspect(&candidate, inspection).await?;
        store
            .ingest_facts(
                &config.tenant_id,
                &config.project_id,
                credential,
                IngestDeliveryFacts {
                    request_id: format!("{}:ingest", request.request_id),
                    read_set: request.read_set,
                    connector_id: config.connector_id.clone(),
                    inspection_id: inspection.into(),
                    event_id: format!("{}:observation", request.request_id),
                    records: snapshot.records,
                },
            )
            .await
            .map_err(error)
    }
}
