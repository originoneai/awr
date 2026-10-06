//! Bind the authenticated command header to one strict neutral domain request.
//! Dispatch before opening the generic command transaction: domain stores own
//! their admission lock, journal, audit event and original receipt.
use super::WorkstreamCommand;
use crate::delivery_sync::*;
use crate::{PgError, PgResult};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

pub(super) const OPERATIONS: &[&str] = &[
    "delivery.connector.configure",
    "delivery.candidate.select",
    "delivery.inspection.reserve",
    "delivery.facts.ingest",
    "delivery.source.prepare",
    "delivery.source.renew",
    "delivery.source.write",
    "delivery.source.confirm",
    "delivery.source.abandon",
    "delivery.integration.prepare",
    "delivery.integration.reject_prepared",
];

pub(super) enum Action {
    Configure(ConfigureDeliveryConnector),
    Select(SelectDeliveryCandidate),
    Reserve(ReserveDeliveryInspection),
    Ingest(IngestDeliveryFacts),
    Prepare(PrepareDeliverySourcePublication),
    Renew(RenewDeliveryPublicationLease),
    Write(DeliveryPublicationStep),
    Confirm(DeliveryPublicationStep),
    Abandon(DeliveryPublicationStep),
    PrepareIntegration(PrepareDeliveryIntegration),
    RejectIntegration(RejectPreparedDeliveryIntegration),
}

fn invalid() -> PgError {
    PgError::invalid_command_fields()
}

fn decode<T: DeserializeOwned>(value: Value) -> PgResult<T> {
    serde_json::from_value(value).map_err(|_| invalid())
}

impl Action {
    pub(super) fn parse(command: &WorkstreamCommand) -> PgResult<Self> {
        let mut args = command.args.as_object().cloned().ok_or_else(invalid)?;
        // Never silently replace an injected identity/read set. Other unknown
        // fields are refused by the domain request's deny_unknown_fields.
        if ["request_id", "read_set", "step"]
            .iter()
            .any(|field| args.contains_key(*field))
        {
            return Err(invalid());
        }
        let source = args.remove("source_snapshot_id").ok_or_else(invalid)?;
        let source = source.as_str().ok_or_else(invalid)?;
        if source.trim().is_empty() || source.len() > 128 || source.chars().any(char::is_control) {
            return Err(invalid());
        }
        let set = DeliveryReadSet {
            work_id: command.work_id.clone(),
            workstream_id: command.workstream_id,
            coordinator_epoch: command.coordinator_epoch.clone(),
            source_snapshot_id: source.into(),
            authority_version: command.expected_authority_version.clone(),
            ownership_version: command.expected_ownership_version.clone(),
            contract_hash: command.expected_contract_hash.clone(),
        };
        args.insert("request_id".into(), json!(command.request_id));
        args.insert("read_set".into(), json!(set));
        if command.op == "delivery.source.renew" {
            let lease = args.remove("lease_seconds").ok_or_else(invalid)?;
            return Ok(Self::Renew(RenewDeliveryPublicationLease {
                step: decode(Value::Object(args))?,
                lease_seconds: decode(lease)?,
            }));
        }
        let value = Value::Object(args);
        Ok(match command.op.as_str() {
            "delivery.connector.configure" => Self::Configure(decode(value)?),
            "delivery.candidate.select" => Self::Select(decode(value)?),
            "delivery.inspection.reserve" => Self::Reserve(decode(value)?),
            "delivery.facts.ingest" => Self::Ingest(decode(value)?),
            "delivery.source.prepare" => Self::Prepare(decode(value)?),
            "delivery.source.write" => Self::Write(decode(value)?),
            "delivery.source.confirm" => Self::Confirm(decode(value)?),
            "delivery.source.abandon" => Self::Abandon(decode(value)?),
            "delivery.integration.prepare" => Self::PrepareIntegration(decode(value)?),
            "delivery.integration.reject_prepared" => Self::RejectIntegration(decode(value)?),
            _ => return Err(invalid()),
        })
    }

    pub(super) async fn execute(
        self,
        store: DeliverySyncStore,
        tenant: &str,
        project: &str,
        bearer: &str,
    ) -> PgResult<Value> {
        let receipt = match self {
            Self::Configure(r) => store.configure_connector(tenant, project, bearer, r).await,
            Self::Select(r) => store.select_candidate(tenant, project, bearer, r).await,
            Self::Reserve(r) => store.reserve_inspection(tenant, project, bearer, r).await,
            Self::Ingest(r) => store.ingest_facts(tenant, project, bearer, r).await,
            Self::Prepare(r) => {
                store
                    .prepare_source_publication(tenant, project, bearer, r)
                    .await
            }
            Self::Renew(r) => {
                store
                    .renew_source_publication(tenant, project, bearer, r)
                    .await
            }
            Self::Write(r) => {
                store
                    .write_source_publication(tenant, project, bearer, r)
                    .await
            }
            Self::Confirm(r) => {
                store
                    .confirm_source_publication(tenant, project, bearer, r)
                    .await
            }
            Self::Abandon(r) => {
                store
                    .abandon_source_publication(tenant, project, bearer, r)
                    .await
            }
            Self::PrepareIntegration(r) => {
                store.prepare_integration(tenant, project, bearer, r).await
            }
            Self::RejectIntegration(r) => {
                store
                    .reject_prepared_integration(tenant, project, bearer, r)
                    .await
            }
        }?;
        Ok(json!({"replayed":receipt["replayed"],"receipt":receipt,
            "execution_authorized":false}))
    }
}
