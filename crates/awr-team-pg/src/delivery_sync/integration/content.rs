//! Resolve proof from the actual observation batch, never a caller argument or head.
use super::*;

pub(super) struct ConfirmationBinding<'a> {
    pub inbox_id: &'a str,
    pub original_connector_version: i64,
    pub observed_connector_version: i64,
    pub observation: &'a IntegrationObservation,
}

pub(super) async fn resolve(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    binding: ConfirmationBinding<'_>,
) -> PgResult<Value> {
    let observation = binding.observation;
    if observation.outcome != IntegrationOutcome::Applied {
        return Ok(Value::Null);
    }
    let source = observation
        .binding
        .source_revision
        .as_ref()
        .ok_or(PgError::EvidenceInvalid)?;
    let result = observation
        .result_revision
        .as_ref()
        .ok_or(PgError::EvidenceInvalid)?;
    if !matches!(
        source.format,
        RevisionFormat::GitSha1 | RevisionFormat::GitSha256
    ) {
        return Err(PgError::EvidenceInvalid);
    }
    // Exact immutable Git object identity preserves the ordinary legacy flow.
    if source == result {
        return Ok(json!({"basis":"exact_revision","fact_id":null}));
    }
    if binding.original_connector_version != binding.observed_connector_version {
        return Err(PgError::EvidenceInvalid);
    }
    let rows = tx
        .query(
            "SELECT id,envelope_json FROM awr_team.delivery_facts
         WHERE tenant_id=$1 AND project_id=$2 AND inbox_id=$3
           AND envelope_json->'record'->>'kind'='integration_content_proof'
         ORDER BY id LIMIT 33",
            &[&tenant, &project, &binding.inbox_id],
        )
        .await?;
    if rows.len() > 32 {
        return Err(PgError::EvidenceInvalid);
    }
    let mut matched = None;
    for row in rows {
        let envelope: DeliveryEnvelope =
            serde_json::from_value(row.get(1)).map_err(|_| PgError::SourceDivergence)?;
        envelope.validate().map_err(|_| PgError::EvidenceInvalid)?;
        let DeliveryRecord::IntegrationContentProof(proof) = envelope.record else {
            return Err(PgError::EvidenceInvalid);
        };
        if proof.observation_reference != observation.external_reference {
            continue;
        }
        if matched.is_some()
            || !proof
                .proves_observation(observation)
                .map_err(|_| PgError::EvidenceInvalid)?
        {
            return Err(PgError::EvidenceInvalid);
        }
        matched = Some(json!({"basis":proof.witness.kind(),"fact_id":row.get::<_,String>(0)}));
    }
    matched.ok_or(PgError::EvidenceInvalid)
}

/// Read the original batch for an already-authorized intent inspection. This
/// preserves historical descriptions without revalidating a terminal receipt.
pub(super) async fn details(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    inbox: &str,
    reference: &str,
) -> PgResult<Value> {
    let rows = tx
        .query(
            "SELECT id,envelope_json FROM awr_team.delivery_facts
         WHERE tenant_id=$1 AND project_id=$2 AND inbox_id=$3
           AND envelope_json->'record'->>'kind'='integration_content_proof'
           AND envelope_json->'record'->'data'->>'observation_reference'=$4
         ORDER BY id LIMIT 2",
            &[&tenant, &project, &inbox, &reference],
        )
        .await?;
    Ok(json!(
        rows.into_iter()
            .map(|r| json!({"fact_id":r.get::<_,String>(0),"envelope":r.get::<_,Value>(1)}))
            .collect::<Vec<_>>()
    ))
}
