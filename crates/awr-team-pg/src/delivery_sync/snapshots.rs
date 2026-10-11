use super::*;
use awr_team::Action;
use serde_json::json;
use tokio_postgres::Transaction;

pub(super) struct InspectionBinding<'a> {
    pub candidate_digest: &'a str,
    pub snapshot: &'a str,
    pub ownership: i64,
    pub selection_version: i64,
}

/// Authenticated writers hold the project lock. Keep this work's fact heads
/// stable until its pending filesystem effect has a known durable outcome.
/// Lease expiry alone cannot release an unknown effect; other work is independent.
pub(super) async fn require_source_publication_settled(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    set: &DeliveryReadSet,
) -> PgResult<()> {
    let pending: bool = tx.query_one("SELECT EXISTS(SELECT 1 FROM awr_team.delivery_source_cursors c
        JOIN awr_team.delivery_source_publications j ON (j.tenant_id,j.project_id,j.id)=(c.tenant_id,c.project_id,c.pending_publication_id)
        WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.source_snapshot_id=$3 AND j.work_id=$4)",
        &[&tenant,&project,&set.source_snapshot_id,&set.work_id]).await?.get(0);
    if pending {
        return Err(PgError::ResourceConflict);
    }
    Ok(())
}

/// Current selection and historical dispatched intents share the same inbox.
/// Admission stays with the caller; this helper grants no effect capability.
#[allow(clippy::too_many_arguments)]
pub(super) async fn reserve_bound(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &crate::workstream_auth::ReaderAuthority,
    request: &ReserveDeliveryInspection,
    connector: &connectors::Connector,
    binding: InspectionBinding<'_>,
    op: &str,
    request_hash: &str,
) -> PgResult<Value> {
    require_source_publication_settled(tx, tenant, project, &request.read_set).await?;
    let generation = connector
        .generation
        .checked_add(1)
        .ok_or(PgError::PreconditionsChanged)?;
    tx.execute("UPDATE awr_team.delivery_connectors SET inspection_generation=$4 WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
        &[&tenant,&project,&request.connector_id,&generation]).await?;
    let id = crate::tx::new_id();
    let row = tx.query_one("INSERT INTO awr_team.delivery_inspections(tenant_id,project_id,id,connector_id,connector_version,
        generation,binding_digest,source_snapshot_id,ownership_version,coordinator_epoch,actor_id,client_id,authority_binding,expires_at,selection_version)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,clock_timestamp()+make_interval(secs=>$14::integer),$15) RETURNING expires_at::text",
        &[&tenant,&project,&id,&request.connector_id,&connector.version,&generation,&binding.candidate_digest,&binding.snapshot,
          &binding.ownership,&auth.epoch,&auth.actor_id,&auth.client_id,&auth::authority_binding(auth,&request.read_set)?,&request.lease_seconds,&binding.selection_version]).await?;
    auth::finish(tx,tenant,project,auth,&request.read_set,op,&request.request_id,request_hash,
        json!({"inspection_id":id,"generation":generation.to_string(),"connector_version":connector.version.to_string(),
            "candidate_digest":binding.candidate_digest,"expires_at":row.get::<_,String>(0),"state":"pending"})).await
}

pub(super) async fn selection(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work: &str,
) -> PgResult<Option<(DeliveryCandidate, String, String, i64, bool, i64)>> {
    let row = tx.query_opt("SELECT c.body_json,s.binding_digest,s.source_snapshot_id,s.ownership_version,
        w.last_fence=s.fence,s.selection_version FROM awr_team.delivery_selections s
        JOIN awr_team.delivery_candidates c USING(tenant_id,project_id,binding_digest)
        JOIN awr_team.work_runtime w ON w.tenant_id=s.tenant_id AND w.project_id=s.project_id AND w.scope_id=s.scope_id AND w.work_id=s.work_id
        WHERE s.tenant_id=$1 AND s.project_id=$2 AND s.scope_id='main' AND s.work_id=$3",
        &[&tenant,&project,&work]).await?;
    row.map(|r| {
        Ok((
            serde_json::from_value(r.get(0)).map_err(|_| PgError::SourceDivergence)?,
            r.get(1),
            r.get(2),
            r.get(3),
            r.get::<_, Option<bool>>(4).unwrap_or(false),
            r.get(5),
        ))
    })
    .transpose()
}

/// Fresh scheduling metadata is outside the immutable original domain receipt.
/// It is not a publish permit; ingest independently rechecks the lease and binding.
async fn with_inspection_lease(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    mut result: Value,
) -> PgResult<Value> {
    let id = result["data"]["inspection_id"].as_str().ok_or(invalid())?;
    let live: bool = tx
        .query_one(
            "SELECT expires_at>clock_timestamp() FROM awr_team.delivery_inspections
        WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &id],
        )
        .await?
        .get(0);
    result["inspection_lease"] = json!({"state_basis":"at_read","live":live});
    Ok(result)
}

impl DeliverySyncStore {
    pub async fn select_candidate(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        request: SelectDeliveryCandidate,
    ) -> PgResult<Value> {
        bounded(&request)?;
        request
            .candidate
            .binding
            .validate()
            .map_err(|_| invalid())?;
        let manifest_digest = request.candidate.manifest.digest().map_err(|_| invalid())?;
        if manifest_digest != request.candidate.binding.manifest_digest {
            return Err(PgError::manifest_digest_mismatch());
        }
        if let Some(value) = &request.expected_selected_digest {
            digest(value)?;
        }
        let set = &request.read_set;
        auth::bind_candidate(tenant, project, set, &request.candidate.binding)?;
        let candidate_digest = request.candidate.binding.digest().map_err(|_| invalid())?;
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
            Some(&request.session_id),
        )
        .await?;
        let op = "delivery.candidate.select";
        let (hash, replayed) = auth::replay(
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
        auth::require_executor(&tx, tenant, project, &auth, &request).await?;
        let checks: Vec<String> = serde_json::from_value(tx.query_one("SELECT contract_json->'verification_requirements'
            FROM awr_team.work_contracts WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND scope_id='main' AND work_id=$4",
            &[&tenant,&project,&auth.snapshot,&set.work_id]).await?.get(0)).map_err(|_| PgError::SourceDivergence)?;
        if checks.iter().collect::<std::collections::BTreeSet<_>>()
            != request.candidate.binding.required_checks.iter().collect()
        {
            return Err(PgError::BindingInvalid);
        }
        let current = selection(&tx, tenant, project, &set.work_id).await?;
        if current.as_ref().map(|s| &s.1) != request.expected_selected_digest.as_ref() {
            return Err(PgError::PreconditionsChanged);
        }
        let selection_version = current
            .as_ref()
            .map(|s| s.5)
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(PgError::PreconditionsChanged)?;
        let binding = &request.candidate.binding;
        let old = tx.query_opt("SELECT binding_digest FROM awr_team.delivery_candidates
            WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND candidate_id=$4 AND candidate_version=$5",
            &[&tenant,&project,&set.work_id,&binding.candidate_id.as_str(),&binding.candidate_version]).await?;
        if old.is_some_and(|r| r.get::<_, String>(0) != candidate_digest) {
            return Err(PgError::IdempotencyConflict);
        }
        tx.execute("INSERT INTO awr_team.delivery_candidates(tenant_id,project_id,binding_digest,work_id,candidate_id,candidate_version,body_json)
            VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(tenant_id,project_id,binding_digest) DO NOTHING",
            &[&tenant,&project,&candidate_digest,&set.work_id,&binding.candidate_id.as_str(),&binding.candidate_version,&json!(request.candidate)]).await?;
        tx.execute("INSERT INTO awr_team.delivery_selections(tenant_id,project_id,scope_id,work_id,binding_digest,source_snapshot_id,
            ownership_version,actor_id,client_id,claim_id,fence,selection_version) VALUES($1,$2,'main',$3,$4,$5,$6,$7,$8,$9,$10,$11)
            ON CONFLICT(tenant_id,project_id,scope_id,work_id) DO UPDATE SET binding_digest=EXCLUDED.binding_digest,
            source_snapshot_id=EXCLUDED.source_snapshot_id,ownership_version=EXCLUDED.ownership_version,
            actor_id=EXCLUDED.actor_id,client_id=EXCLUDED.client_id,claim_id=EXCLUDED.claim_id,fence=EXCLUDED.fence,selection_version=EXCLUDED.selection_version",
            &[&tenant,&project,&set.work_id,&candidate_digest,&auth.snapshot,&version(&set.ownership_version)?,
              &auth.actor_id,&auth.client_id,&request.claim_id,&version(&request.fence)?,&selection_version]).await?;
        let result = auth::finish(
            &tx,
            tenant,
            project,
            &auth,
            set,
            op,
            &request.request_id,
            &hash,
            json!({"candidate_digest":candidate_digest,"candidate_id":binding.candidate_id,
                "candidate_version":binding.candidate_version,"selection_version":selection_version.to_string(),"state":"selected"}),
        )
        .await?;
        tx.commit().await?;
        Ok(result)
    }

    /// Service-assigned generation orders reconciliation; provider timestamps do not.
    pub async fn reserve_inspection(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        request: ReserveDeliveryInspection,
    ) -> PgResult<Value> {
        bounded(&request)?;
        digest(&request.candidate_digest)?;
        if !(5..=300).contains(&request.lease_seconds) {
            return Err(invalid());
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
        if connector.version != version(&request.connector_version)? {
            return Err(PgError::PreconditionsChanged);
        }
        let op = "delivery.inspection.reserve";
        let (hash, replayed) = auth::replay(
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
            let result = with_inspection_lease(&tx, tenant, project, result).await?;
            tx.commit().await?;
            return Ok(result);
        }
        let (candidate, selected, snapshot, ownership, fence_current, selection_version) =
            selection(&tx, tenant, project, &set.work_id)
                .await?
                .ok_or(PgError::InactiveCandidate)?;
        auth::bind_candidate(tenant, project, set, &candidate.binding)?;
        if selected != request.candidate_digest
            || snapshot != auth.snapshot
            || ownership != version(&set.ownership_version)?
            || !fence_current
            || candidate.binding.target.resource != connector.resource
            || candidate
                .binding
                .source_revision
                .as_ref()
                .is_some_and(|r| r.resource != connector.resource)
        {
            return Err(PgError::PreconditionsChanged);
        }
        let result = reserve_bound(
            &tx,
            tenant,
            project,
            &auth,
            &request,
            &connector,
            InspectionBinding {
                candidate_digest: &selected,
                snapshot: &auth.snapshot,
                ownership,
                selection_version,
            },
            op,
            &hash,
        )
        .await?;
        let result = with_inspection_lease(&tx, tenant, project, result).await?;
        tx.commit().await?;
        Ok(result)
    }

    /// Bounded current and historical descriptions. This read grants no execution or acceptance.
    pub async fn inspect(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        work: &str,
    ) -> PgResult<Value> {
        identity(work)?;
        // A write to the project between this read's snapshot and its locks makes PostgreSQL
        // roll the read back (serialization failure). It changes nothing, so it runs again.
        crate::retry_rolled_back(|| self.inspect_once(tenant, project, bearer, work)).await
    }

    async fn inspect_once(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        work: &str,
    ) -> PgResult<Value> {
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let mut auth = crate::workstream_auth::authenticate(&tx, tenant, project, bearer).await?;
        crate::delegation_auth::resolve_agent_delegation(
            &tx,
            &mut auth,
            project,
            Some(work),
            None,
            Some(Action::WorkRead),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
        )
        .await?;
        let result = Self::inspect_in_tx(&tx, tenant, project, &auth, work).await?;
        tx.commit().await?;
        Ok(result)
    }

    pub(crate) async fn describe_submission_in_tx(
        tx: &Transaction<'_>,
        tenant: &str,
        project: &str,
        auth: &crate::workstream_auth::ReaderAuthority,
        work: &str,
    ) -> PgResult<Value> {
        let (binding, ownership) =
            crate::workstream_read::work_binding(tx, tenant, project, auth, work).await?;
        crate::workstream_auth::authorize_domain_action(
            auth,
            Action::WorkRead,
            Some(binding.workstream_id),
            Some(work),
        )?;
        let row = tx
            .query_one(
                "SELECT contract_hash,contract_json->'verification_requirements'
            FROM awr_team.work_contracts WHERE tenant_id=$1 AND project_id=$2
            AND snapshot_id=$3 AND scope_id='main' AND work_id=$4",
                &[&tenant, &project, &auth.snapshot, &work],
            )
            .await?;
        let contract: String = row.get(0);
        let checks: Value = row.get(1);
        let selected = selection(tx, tenant, project, work).await?;
        let current = selected
            .as_ref()
            .is_some_and(|(candidate, _, snapshot, owner, fence, _)| {
                *snapshot == auth.snapshot
                    && *owner == ownership
                    && *fence
                    && candidate.binding.contract_hash == contract
            });
        let (connectors, truncated) = submission_connectors(
            tx,
            tenant,
            project,
            work,
            binding.workstream_id,
            &auth.epoch,
        )
        .await?;
        // Input discovery reads no artifact bytes, evidence, review rounds or fact history.
        super::submission_contract::describe(json!({
            "work_id":work,"workstream_id":binding.workstream_id,
            "source_snapshot_id":auth.snapshot,"coordinator_epoch":auth.epoch,
            "project_revision":auth.revision.to_string(),"selected_current":current,
            "selection_version":selected.as_ref().map(|s|s.5.to_string()),
            "candidate":selected.map(|s|s.0),"connectors":connectors,"connectors_truncated":truncated,
            "submission":{"candidate_context":super::submission_contract::context(
                tenant,project,binding.workstream_id,work,&contract,checks)}}))
    }

    pub(crate) async fn inspect_in_tx(
        tx: &Transaction<'_>,
        tenant: &str,
        project: &str,
        auth: &crate::workstream_auth::ReaderAuthority,
        work: &str,
    ) -> PgResult<Value> {
        let (binding, ownership) =
            crate::workstream_read::work_binding(tx, tenant, project, auth, work).await?;
        crate::workstream_auth::authorize_domain_action(
            auth,
            Action::WorkRead,
            Some(binding.workstream_id),
            Some(work),
        )?;
        let selected = selection(tx, tenant, project, work).await?;
        let current_contract: String = tx.query_one("SELECT contract_hash FROM awr_team.work_contracts
            WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND scope_id='main' AND work_id=$4",
            &[&tenant,&project,&auth.snapshot,&work]).await?.get(0);
        let selected_current = selected.as_ref().is_some_and(|(c, _, s, o, f, _)| {
            *s == auth.snapshot
                && *o == ownership
                && *f
                && c.binding.contract_hash == current_contract
        });
        let mut facts = Vec::new();
        let rows = tx.query("SELECT f.envelope_json,f.id,i.receipt_json,x.binding_digest,x.source_snapshot_id,
            x.ownership_version,x.coordinator_epoch,c.version=x.connector_version AND c.enabled AND c.coordinator_epoch=x.coordinator_epoch,x.selection_version
            FROM awr_team.delivery_fact_heads h JOIN awr_team.delivery_facts f ON f.tenant_id=h.tenant_id AND f.project_id=h.project_id AND f.id=h.fact_id
            JOIN awr_team.delivery_inbox i ON i.tenant_id=f.tenant_id AND i.project_id=f.project_id AND i.id=f.inbox_id
            JOIN awr_team.delivery_inspections x ON x.tenant_id=i.tenant_id AND x.project_id=i.project_id AND x.id=i.inspection_id
            JOIN awr_team.delivery_connectors c ON c.tenant_id=h.tenant_id AND c.project_id=h.project_id AND c.id=h.connector_id
            WHERE h.tenant_id=$1 AND h.project_id=$2 AND h.work_id=$3 ORDER BY h.connector_id,h.slot LIMIT 33",
            &[&tenant,&project,&work]).await?;
        let facts_truncated = rows.len() > 32;
        for row in rows.into_iter().take(32) {
            let envelope: DeliveryEnvelope =
                serde_json::from_value(row.get(0)).map_err(|_| PgError::SourceDivergence)?;
            let is_current = selected_current
                && selected
                    .as_ref()
                    .is_some_and(|s| s.1 == row.get::<_, String>(3))
                && row.get::<_, String>(4) == auth.snapshot
                && row.get::<_, i64>(5) == ownership
                && row.get::<_, String>(6) == auth.epoch
                && row.get::<_, bool>(7)
                && selected
                    .as_ref()
                    .is_some_and(|s| s.5 == row.get::<_, i64>(8));
            facts.push(
                json!({"fact_id":row.get::<_,String>(1),"current":is_current,
                "observation":inbox::summary(&envelope),"receipt":row.get::<_,Value>(2)}),
            );
        }
        let rows = tx.query("SELECT i.receipt_json FROM awr_team.delivery_inbox i JOIN awr_team.delivery_connectors c
            ON c.tenant_id=i.tenant_id AND c.project_id=i.project_id AND c.id=i.connector_id
            WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.work_id=$3 ORDER BY i.recorded_at DESC,i.id DESC LIMIT 33",
            &[&tenant,&project,&work]).await?;
        let history_truncated = rows.len() > 32;
        let history: Vec<Value> = rows.into_iter().take(32).map(|r| r.get(0)).collect();
        // Work-scoped descriptions let supervisors obtain integration preconditions.
        // They reveal no credential or principal and grant no worker capability.
        let (connectors, connectors_truncated) = submission_connectors(
            tx,
            tenant,
            project,
            work,
            binding.workstream_id,
            &auth.epoch,
        )
        .await?;
        let evidence = tx
            .query_opt(
                "SELECT id FROM awr_team.evidence
            WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND contract_hash=$4
            ORDER BY created_at DESC,id DESC LIMIT 1",
                &[&tenant, &project, &work, &current_contract],
            )
            .await?
            .map(|r| r.get::<_, String>(0));
        let submission = super::review_submission::inspect(
            tx,
            tenant,
            project,
            auth,
            work,
            evidence.as_deref(),
            None,
        )
        .await?;
        let mut result = json!({"work_id":work,"workstream_id":binding.workstream_id,"source_snapshot_id":auth.snapshot,
            "coordinator_epoch":auth.epoch,"project_revision":auth.revision.to_string(),"selected_current":selected_current,
            "selection_version":selected.as_ref().map(|s|s.5.to_string()),"candidate":selected.map(|s|s.0),"facts":facts,"history":history,
            "facts_truncated":facts_truncated,"history_truncated":history_truncated,
            "connectors":connectors,"connectors_truncated":connectors_truncated,
            "submission":submission,
            "acceptance_ready":false,"execution_authorized":false,"source_synchronized":false});
        result["submission"]["describe_query"] = json!({"protocol_version":1,
            "op":"delivery.submission.describe","work_id":work});
        if result["submission"]["delivery_required"] == true {
            let checks: Value = tx
                .query_one(
                    "SELECT contract_json->'verification_requirements'
                FROM awr_team.work_contracts WHERE tenant_id=$1 AND project_id=$2
                AND snapshot_id=$3 AND scope_id='main' AND work_id=$4",
                    &[&tenant, &project, &auth.snapshot, &work],
                )
                .await?
                .get(0);
            result["submission"]["candidate_context"] = super::submission_contract::context(
                tenant,
                project,
                binding.workstream_id,
                work,
                &current_contract,
                checks,
            );
        }
        if serde_json::to_vec(&result).map_err(|_| invalid())?.len() > 262144 {
            return Err(PgError::ResponseTooLarge);
        }
        Ok(result)
    }
}

async fn submission_connectors(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work: &str,
    workstream: Id,
    epoch: &str,
) -> PgResult<(Vec<Value>, bool)> {
    let rows = tx
        .query(
            "SELECT id,version,provider,resource,fact_source,enabled,coordinator_epoch
        FROM awr_team.delivery_connectors WHERE tenant_id=$1 AND project_id=$2
        AND work_id=$3 AND workstream_id=$4 ORDER BY id LIMIT 33",
            &[&tenant, &project, &work, &workstream.to_string()],
        )
        .await?;
    let truncated = rows.len() > 32;
    let connectors = rows.into_iter().take(32).map(|r| json!({
        "connector_id":r.get::<_,String>(0),"connector_version":r.get::<_,i64>(1).to_string(),
        "provider":r.get::<_,String>(2),"resource":r.get::<_,String>(3),
        "fact_source":r.get::<_,String>(4),"enabled":r.get::<_,bool>(5),
        "current_epoch":r.get::<_,String>(6)==epoch
    })).collect();
    Ok((connectors, truncated))
}
