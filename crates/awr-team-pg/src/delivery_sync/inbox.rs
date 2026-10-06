use super::*;
use awr_team::{Action, delivery::DeliveryRecord};
use serde_json::json;
use std::collections::BTreeSet;

fn slot(record: &DeliveryRecord) -> PgResult<String> {
    hash(&match record {
        DeliveryRecord::ChangeRequest(r) => json!(["change_request", r.provider, r.resource_id]),
        DeliveryRecord::Verification(r) => json!(["verification", r.check]),
        DeliveryRecord::ReviewDecision(r) => json!(["review_reference", r.round_id, r.decision_id]),
        DeliveryRecord::IntegrationRequest(r) => {
            json!(["integration_intent_reference", r.request_id])
        }
        DeliveryRecord::IntegrationObservation(r) => {
            json!(["integration_observation", r.external_reference])
        }
        _ => return Err(invalid()),
    })
}

fn normalize(
    envelope: &mut DeliveryEnvelope,
    source: &FactSource,
    recorded_at: u64,
) -> PgResult<()> {
    let provenance = match &mut envelope.record {
        DeliveryRecord::ChangeRequest(r) => Some(&mut r.provenance),
        DeliveryRecord::Verification(r) => Some(&mut r.provenance),
        DeliveryRecord::IntegrationObservation(r) => Some(&mut r.provenance),
        DeliveryRecord::ReviewDecision(_) | DeliveryRecord::IntegrationRequest(_) => None,
        _ => return Err(invalid()),
    };
    if let Some(p) = provenance {
        if &p.source != source {
            return Err(PgError::Forbidden);
        }
        p.recorded_at_unix_ms = recorded_at;
        // Unknown observation time stays unknown; server receipt time is distinct.
    }
    envelope.validate().map_err(|_| invalid())
}

pub(super) fn summary(envelope: &DeliveryEnvelope) -> Value {
    match &envelope.record {
        DeliveryRecord::ChangeRequest(r) => json!({"kind":"change_request","provider":r.provider,
            "resource_id":r.resource_id,"provenance":r.provenance}),
        DeliveryRecord::Verification(r) => {
            json!({"kind":"verification","check":r.check,"run_id":r.run_id,
            "outcome":r.outcome,"provenance":r.provenance})
        }
        DeliveryRecord::ReviewDecision(r) => {
            json!({"kind":"review_reference","round_id":r.round_id,"decision_id":r.decision_id,
            "resolved":false})
        }
        DeliveryRecord::IntegrationRequest(r) => {
            json!({"kind":"integration_intent_reference","request_id":r.request_id,
            "resolved":false})
        }
        DeliveryRecord::IntegrationObservation(r) => {
            json!({"kind":"integration_observation","external_reference":r.external_reference,
            "outcome":r.outcome,"result_revision":r.result_revision,"provenance":r.provenance})
        }
        _ => json!({"kind":"unsupported"}),
    }
}

impl DeliverySyncStore {
    /// Query, poll and optional webhook wakeups all reconcile through this inbox.
    /// This method consumes a reserved observation, never the notification body itself.
    pub async fn ingest_facts(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        request: IngestDeliveryFacts,
    ) -> PgResult<Value> {
        bounded(&request)?;
        for id in [&request.inspection_id, &request.event_id] {
            identity(id)?;
        }
        if request.records.is_empty() || request.records.len() > 32 {
            return Err(invalid());
        }
        let mut slots = BTreeSet::new();
        for record in &request.records {
            record.validate().map_err(|_| invalid())?;
            if !slots.insert(slot(&record.record)?) {
                return Err(invalid());
            }
        }
        let set = &request.read_set;
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let auth = auth::admit(
            &tx,
            tenant,
            project,
            bearer,
            set,
            Action::DeliverySubmitAndRequestReview,
            None,
        )
        .await?;
        let connector =
            connectors::load(&tx, tenant, project, &auth, set, &request.connector_id).await?;
        let reserved = tx
            .query_opt(
                "SELECT x.connector_version,x.generation,x.binding_digest,x.source_snapshot_id,
            x.ownership_version,x.coordinator_epoch,x.actor_id,x.client_id,x.authority_binding,
            x.expires_at>clock_timestamp(),c.body_json,x.selection_version FROM awr_team.delivery_inspections x
            JOIN awr_team.delivery_candidates c USING(tenant_id,project_id,binding_digest)
            WHERE x.tenant_id=$1 AND x.project_id=$2 AND x.id=$3 AND x.connector_id=$4",
                &[
                    &tenant,
                    &project,
                    &request.inspection_id,
                    &request.connector_id,
                ],
            )
            .await?
            .ok_or(PgError::Forbidden)?;
        if reserved.get::<_, String>(6) != auth.actor_id
            || reserved.get::<_, String>(7) != auth.client_id
            || reserved.get::<_, String>(8) != auth::authority_binding(&auth, set)?
        {
            return Err(PgError::Forbidden);
        }
        if reserved.get::<_, String>(5) != auth.epoch {
            return Err(PgError::EpochChanged);
        }
        if reserved.get::<_, i64>(0) != connector.version {
            return Err(PgError::PreconditionsChanged);
        }
        let candidate: DeliveryCandidate =
            serde_json::from_value(reserved.get(10)).map_err(|_| PgError::SourceDivergence)?;
        for envelope in &request.records {
            envelope
                .record
                .validate_against(&candidate.binding)
                .map_err(|_| PgError::BindingInvalid)?;
        }
        let op = "delivery.facts.ingest";
        let (request_hash, replayed) = auth::replay(
            &tx,
            tenant,
            project,
            &auth,
            op,
            &request.request_id,
            &request,
        )
        .await?;
        if let Some(result) = replayed {
            tx.commit().await?;
            return Ok(result);
        }
        let input_digest =
            hash(&json!({"inspection_id":request.inspection_id,"records":request.records}))?;
        if let Some(row) = tx
            .query_opt(
                "SELECT input_digest,receipt_json FROM awr_team.delivery_inbox
            WHERE tenant_id=$1 AND project_id=$2 AND connector_id=$3 AND event_id=$4",
                &[&tenant, &project, &request.connector_id, &request.event_id],
            )
            .await?
        {
            if row.get::<_, String>(0) != input_digest {
                return Err(PgError::IdempotencyConflict);
            }
            let result = auth::finish(
                &tx,
                tenant,
                project,
                &auth,
                set,
                op,
                &request.request_id,
                &request_hash,
                json!({"event_replayed":true,"observation_receipt":row.get::<_,Value>(1)}),
            )
            .await?;
            tx.commit().await?;
            return Ok(result);
        }
        let generation: i64 = reserved.get(1);
        let binding_digest: String = reserved.get(2);
        let source_snapshot: String = reserved.get(3);
        let ownership: i64 = reserved.get(4);
        let selection_version: i64 = reserved.get(11);
        let selected = snapshots::selection(&tx, tenant, project, &set.work_id).await?;
        let current = generation == connector.generation
            && reserved.get::<_, bool>(9)
            && source_snapshot == auth.snapshot
            && ownership == version(&set.ownership_version)?
            && selected.as_ref().is_some_and(|s| {
                s.1 == binding_digest
                    && s.2 == auth.snapshot
                    && s.3 == ownership
                    && s.4
                    && s.5 == selection_version
            })
            && candidate.binding.contract_hash == set.contract_hash;
        let state = if current { "applied" } else { "superseded" };
        let at: i64 = tx
            .query_one(
                "SELECT (EXTRACT(EPOCH FROM clock_timestamp())*1000)::bigint",
                &[],
            )
            .await?
            .get(0);
        let mut records = request.records.clone();
        for record in &mut records {
            normalize(record, &connector.source, at as u64)?;
        }
        let inbox_id = crate::tx::new_id();
        let notification_id = crate::tx::new_id();
        let fact_ids: Vec<_> = records.iter().map(|_| crate::tx::new_id()).collect();
        let receipt = json!({"receipt_id":inbox_id,"connector_id":request.connector_id,"event_id":request.event_id,
            "input_digest":input_digest,"inspection_id":request.inspection_id,"generation":generation.to_string(),
            "candidate_digest":binding_digest,"selection_version":selection_version.to_string(),"source_snapshot_id":source_snapshot,"state":state,
            "fact_source":connector.source,"recorded_at_unix_ms":at,"fact_ids":fact_ids,"notification_id":notification_id,
            "acceptance_ready":false,"source_synchronized":false});
        tx.execute("INSERT INTO awr_team.delivery_inbox(tenant_id,project_id,id,connector_id,event_id,inspection_id,input_digest,state,receipt_json)
            VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)",
            &[&tenant,&project,&inbox_id,&request.connector_id,&request.event_id,&request.inspection_id,&input_digest,&state,&receipt]).await?;
        for (record, id) in records.iter().zip(&fact_ids) {
            let slot = slot(&record.record)?;
            tx.execute("INSERT INTO awr_team.delivery_facts(tenant_id,project_id,id,inbox_id,slot,envelope_json) VALUES($1,$2,$3,$4,$5,$6)",
                &[&tenant,&project,&id,&inbox_id,&slot,&json!(record)]).await?;
            if current {
                let updated = tx.execute("INSERT INTO awr_team.delivery_fact_heads(tenant_id,project_id,connector_id,work_id,slot,generation,fact_id)
                    VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(tenant_id,project_id,connector_id,slot) DO UPDATE
                    SET generation=EXCLUDED.generation,fact_id=EXCLUDED.fact_id WHERE awr_team.delivery_fact_heads.generation<EXCLUDED.generation",
                    &[&tenant,&project,&request.connector_id,&set.work_id,&slot,&generation,&id]).await?;
                if updated != 1 {
                    return Err(PgError::IdempotencyConflict);
                }
            }
        }
        tx.execute("INSERT INTO awr_team.delivery_notifications(tenant_id,project_id,id,inbox_id,work_id,state) VALUES($1,$2,$3,$4,$5,$6)",
            &[&tenant,&project,&notification_id,&inbox_id,&set.work_id,&if current {"pending"} else {"superseded"}]).await?;
        if current {
            pump::enqueue(
                &tx,
                tenant,
                project,
                &notification_id,
                &request,
                connector.version,
                generation,
                &binding_digest,
                selection_version,
            )
            .await?;
        }
        let result = auth::finish(
            &tx,
            tenant,
            project,
            &auth,
            set,
            op,
            &request.request_id,
            &request_hash,
            json!({"event_replayed":false,"observation_receipt":receipt}),
        )
        .await?;
        tx.commit().await?;
        Ok(result)
    }
}
