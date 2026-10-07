//! Durable neutral observations and recoverable, confined source metadata writes.
//! Repository effects, approval and domain finalization remain separate.
mod auth;
pub(crate) mod completion;
mod connectors;
mod inbox;
mod integration;
mod publisher;
mod pump;
mod scheduling;
mod snapshots;

pub use integration::{
    ConfirmDeliveryIntegration, DeliveryIntegrationDispatch, DeliveryIntegrationPermit,
    DispatchDeliveryIntegration, LeaseDeliveryIntegration, PrepareDeliveryIntegration,
    RejectPreparedDeliveryIntegration,
};
pub use publisher::{
    DeliveryPublicationStep, PrepareDeliverySourcePublication, RenewDeliveryPublicationLease,
};
pub use pump::{ClaimDeliverySyncIntent, DeliverySyncLease};
pub use scheduling::DeliveryScheduleQuery;

use crate::{PgError, PgPool, PgResult};
use awr_core::Id;
use awr_team::delivery::{DeliveryCandidate, DeliveryEnvelope, FactSource};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryReadSet {
    pub work_id: String,
    pub workstream_id: Id,
    pub coordinator_epoch: String,
    pub source_snapshot_id: String,
    pub authority_version: String,
    pub ownership_version: String,
    pub contract_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryConnectorMapping {
    pub connector_id: String,
    pub provider: String,
    /// Opaque repository/artifact identity, never a credential or arbitrary payload.
    pub resource: String,
    pub principal_actor_id: String,
    pub principal_client_id: String,
    pub fact_source: FactSource,
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigureDeliveryConnector {
    pub request_id: String,
    pub read_set: DeliveryReadSet,
    /// Zero creates a new mapping; every replacement/revocation requires its version.
    pub expected_connector_version: String,
    pub mapping: DeliveryConnectorMapping,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectDeliveryCandidate {
    pub request_id: String,
    pub read_set: DeliveryReadSet,
    pub expected_selected_digest: Option<String>,
    pub session_id: String,
    pub claim_id: String,
    pub fence: String,
    pub lease_version: String,
    pub candidate: DeliveryCandidate,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReserveDeliveryInspection {
    pub request_id: String,
    pub read_set: DeliveryReadSet,
    pub connector_id: String,
    pub connector_version: String,
    pub candidate_digest: String,
    pub lease_seconds: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IngestDeliveryFacts {
    pub request_id: String,
    /// Current authority, which may differ from an old inspection's source snapshot.
    pub read_set: DeliveryReadSet,
    pub connector_id: String,
    pub inspection_id: String,
    pub event_id: String,
    /// One bounded atomic observation batch. No untyped provider payload is retained.
    pub records: Vec<DeliveryEnvelope>,
}

pub struct DeliverySyncStore {
    pool: Arc<PgPool>,
}

impl DeliverySyncStore {
    pub fn new(url: impl Into<String>) -> Self {
        Self::from_pool(Arc::new(PgPool::new(url)))
    }

    pub fn from_config(config: tokio_postgres::Config) -> Self {
        Self::from_pool(Arc::new(PgPool::from_config(config)))
    }

    pub(crate) fn from_pool(pool: Arc<PgPool>) -> Self {
        Self { pool }
    }
}

fn invalid() -> PgError {
    PgError::Protocol("invalid delivery synchronization fields or bounds".into())
}

fn identity(value: &str) -> PgResult<()> {
    text(value, 128)
}

fn text(value: &str, max: usize) -> PgResult<()> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(invalid());
    }
    Ok(())
}

fn version(value: &str) -> PgResult<i64> {
    if value.is_empty()
        || value.len() > 19
        || value.len() > 1 && value.starts_with('0')
        || !value.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(invalid());
    }
    value.parse().map_err(|_| invalid())
}

fn digest(value: &str) -> PgResult<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid());
    }
    Ok(())
}

fn bounded<T: Serialize>(value: &T) -> PgResult<()> {
    if serde_json::to_vec(value).map_err(|_| invalid())?.len() > 65536 {
        return Err(invalid());
    }
    awr_core::ensure_public_data(value).map_err(|_| {
        PgError::Protocol("delivery synchronization accepts only redacted public facts".into())
    })
}

fn hash(value: &Value) -> PgResult<String> {
    awr_team::request_hash(value).map_err(|_| invalid())
}
