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

/// Reuse only a current, mapped adapter's inclusion proof for this exact selection.
/// Historical integration confirmations alone do not describe today's target.
pub(in crate::delivery_sync) async fn current_target_contains(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    set: &DeliveryReadSet,
    candidate: &DeliveryCandidate,
    selection: i64,
) -> PgResult<bool> {
    let rows = tx.query("SELECT f.envelope_json,i.id,x.connector_version FROM awr_team.delivery_fact_heads h
        JOIN awr_team.delivery_facts f ON (f.tenant_id,f.project_id,f.id)=(h.tenant_id,h.project_id,h.fact_id)
        JOIN awr_team.delivery_inbox i ON (i.tenant_id,i.project_id,i.id)=(f.tenant_id,f.project_id,f.inbox_id)
        JOIN awr_team.delivery_inspections x ON (x.tenant_id,x.project_id,x.id)=(i.tenant_id,i.project_id,i.inspection_id)
        JOIN awr_team.delivery_connectors c ON (c.tenant_id,c.project_id,c.id)=(h.tenant_id,h.project_id,h.connector_id)
        WHERE h.tenant_id=$1 AND h.project_id=$2 AND h.work_id=$3 AND i.state='applied'
          AND x.binding_digest=$4 AND x.source_snapshot_id=$5 AND x.ownership_version=$6
          AND x.selection_version=$7 AND x.coordinator_epoch=$8 AND c.enabled
          AND c.version=x.connector_version AND c.coordinator_epoch=x.coordinator_epoch
          AND c.fact_source='adapter_observation'
          AND f.envelope_json#>>'{record,kind}'='integration_observation'
        ORDER BY h.connector_id,h.slot LIMIT 33",
        &[&tenant,&project,&set.work_id,&candidate.binding.digest().map_err(|_| PgError::BindingInvalid)?,
            &set.source_snapshot_id,&version(&set.ownership_version)?,&selection,&set.coordinator_epoch]).await?;
    if rows.len() > 32 {
        return Err(PgError::ResponseTooLarge);
    }
    for row in rows {
        let envelope: DeliveryEnvelope =
            serde_json::from_value(row.get(0)).map_err(|_| PgError::SourceDivergence)?;
        envelope.validate().map_err(|_| PgError::EvidenceInvalid)?;
        envelope
            .record
            .validate_against(&candidate.binding)
            .map_err(|_| PgError::BindingInvalid)?;
        let DeliveryRecord::IntegrationObservation(observation) = envelope.record else {
            return Err(PgError::SourceDivergence);
        };
        if observation.provenance.source != FactSource::AdapterObservation
            || observation.outcome != IntegrationOutcome::Applied
        {
            continue;
        }
        let inbox: String = row.get(1);
        let connector_version: i64 = row.get(2);
        match resolve(
            tx,
            tenant,
            project,
            ConfirmationBinding {
                inbox_id: &inbox,
                original_connector_version: connector_version,
                observed_connector_version: connector_version,
                observation: &observation,
            },
        )
        .await
        {
            Ok(proof) if !proof.is_null() => return Ok(true),
            Ok(_) | Err(PgError::EvidenceInvalid) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(false)
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
