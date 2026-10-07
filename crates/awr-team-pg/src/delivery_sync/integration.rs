//! Version-bound authority for one repository effect, separate from acceptance.
mod content;
mod eligibility;
mod lifecycle;

use super::*;
use crate::workstream_auth::{CredentialReference, ReaderAuthority, credential_reference};
use awr_team::{Action, delivery::*};
use serde_json::json;
use tokio_postgres::Transaction;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrepareDeliveryIntegration {
    pub request_id: String,
    pub read_set: DeliveryReadSet,
    pub connector_id: String,
    pub connector_version: String,
    pub candidate_digest: String,
    pub selection_version: String,
    pub evidence_id: String,
    pub review_round_id: String,
    pub review_decision_id: String,
    pub operation: IntegrationOperation,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseDeliveryIntegration {
    pub request_id: String,
    pub read_set: DeliveryReadSet,
    pub integration_id: String,
    pub lease_seconds: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchDeliveryIntegration {
    pub request_id: String,
    pub read_set: DeliveryReadSet,
    pub integration_id: String,
    pub lease_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmDeliveryIntegration {
    pub request_id: String,
    pub read_set: DeliveryReadSet,
    pub integration_id: String,
    pub fact_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RejectPreparedDeliveryIntegration {
    pub request_id: String,
    pub read_set: DeliveryReadSet,
    pub integration_id: String,
    pub reason: String,
}

/// Public receipts are descriptions. Only the first dispatch contains a permit.
pub struct DeliveryIntegrationDispatch {
    pub receipt: Value,
    pub permit: Option<DeliveryIntegrationPermit>,
}

/// Not cloneable or deserializable: a replay cannot construct another effect.
/// Inspection getters expose the binding, never the original credential reference.
pub struct DeliveryIntegrationPermit {
    candidate: DeliveryCandidate,
    request: IntegrationRequest,
    read_set: DeliveryReadSet,
    connector_id: String,
    connector_version: String,
    eligibility_digest: String,
}

impl DeliveryIntegrationPermit {
    pub fn candidate(&self) -> &DeliveryCandidate {
        &self.candidate
    }
    pub fn request(&self) -> &IntegrationRequest {
        &self.request
    }
    pub fn read_set(&self) -> &DeliveryReadSet {
        &self.read_set
    }
    pub fn connector_id(&self) -> &str {
        &self.connector_id
    }
    pub fn connector_version(&self) -> &str {
        &self.connector_version
    }
    pub fn eligibility_digest(&self) -> &str {
        &self.eligibility_digest
    }
}

struct Eligibility {
    candidate: DeliveryCandidate,
    checks: Vec<VerificationRef>,
    binding: Value,
}

fn command(
    auth: &ReaderAuthority,
    set: &DeliveryReadSet,
    request_id: &str,
) -> crate::WorkstreamCommand {
    crate::WorkstreamCommand {
        protocol_version: 1,
        request_id: request_id.into(),
        op: "delivery.finalize".into(),
        work_id: set.work_id.clone(),
        workstream_id: set.workstream_id,
        coordinator_epoch: set.coordinator_epoch.clone(),
        expected_project_revision: auth.revision.to_string(),
        expected_authority_version: set.authority_version.clone(),
        expected_ownership_version: set.ownership_version.clone(),
        expected_contract_hash: set.contract_hash.clone(),
        args: json!({}),
    }
}
