//! Authenticated observations for optional provider workers. Never an effect permit.
use super::*;
use awr_team::{Action, WorkContract, delivery::DeliveryRecord};
use serde_json::json;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryScheduleQuery {
    pub work_id: String,
    pub connector_id: String,
    /// A continuation from this exact principal, mapping and current source binding.
    pub cursor: Option<String>,
    pub limit: i32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    binding: String,
    after_id: String,
    upper_id: String,
}

const MAX_RESPONSE_BYTES: usize = 262144;

fn encode_cursor(binding: &str, after_id: &str, upper_id: &str) -> PgResult<String> {
    serde_json::to_string(&Cursor {
        binding: binding.into(),
        after_id: after_id.into(),
        upper_id: upper_id.into(),
    })
    .map_err(|_| invalid())
}

impl DeliverySyncStore {
    /// Read current authority and original integration history in one transaction.
    /// The query accepts no caller-supplied version, identity or approval flag.
    pub async fn schedule(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        query: DeliveryScheduleQuery,
    ) -> PgResult<Value> {
        bounded(&query)?;
        identity(&query.work_id)?;
        identity(&query.connector_id)?;
        if !(1..=32).contains(&query.limit) {
            return Err(invalid());
        }
        if query.cursor.as_ref().is_some_and(|c| c.len() > 4096) {
            return Err(PgError::CursorExpired);
        }
        // A write to the project between this read's snapshot and its locks makes PostgreSQL
        // roll the read back (serialization failure). It changes nothing, so it runs again.
        crate::retry_rolled_back(|| self.schedule_once(tenant, project, bearer, &query)).await
    }

    async fn schedule_once(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        query: &DeliveryScheduleQuery,
    ) -> PgResult<Value> {
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let auth = crate::workstream_auth::authenticate(&tx, tenant, project, bearer).await?;
        if auth.actor_kind != "system" {
            return Err(PgError::Forbidden);
        }
        if auth.project_status != "active" {
            return Err(PgError::ProjectNotAvailable);
        }
        let (work, ownership) =
            crate::workstream_read::work_binding(&tx, tenant, project, &auth, &query.work_id)
                .await?;
        crate::workstream_auth::authorize_domain_action(
            &auth,
            Action::WorkRead,
            Some(work.workstream_id),
            Some(&query.work_id),
        )?;
        let row = tx
            .query_opt(
                "SELECT contract_json,contract_hash FROM awr_team.work_contracts
                WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3
                  AND scope_id='main' AND work_id=$4",
                &[&tenant, &project, &auth.snapshot, &query.work_id],
            )
            .await?
            .ok_or(PgError::SourceDivergence)?;
        let contract: WorkContract =
            serde_json::from_value(row.get(0)).map_err(|_| PgError::SourceDivergence)?;
        let contract_hash: String = row.get(1);
        if contract.work_id.as_str() != query.work_id
            || contract.hash().map_err(|_| PgError::SourceDivergence)? != contract_hash
        {
            return Err(PgError::SourceDivergence);
        }
        let set = DeliveryReadSet {
            work_id: query.work_id.clone(),
            workstream_id: work.workstream_id,
            coordinator_epoch: auth.epoch.clone(),
            source_snapshot_id: auth.snapshot.clone(),
            authority_version: auth
                .catalog
                .get(work.workstream_id)?
                .authority_version
                .to_string(),
            ownership_version: ownership.to_string(),
            contract_hash,
        };
        let connector = tx.query_opt("SELECT work_id,workstream_id,principal_actor_id,principal_client_id,
            coordinator_epoch,version,fact_source,resource,enabled,provider FROM awr_team.delivery_connectors
            WHERE tenant_id=$1 AND project_id=$2 AND id=$3 FOR SHARE",
            &[&tenant,&project,&query.connector_id]).await?.ok_or(PgError::Forbidden)?;
        if connector.get::<_, String>(0) != query.work_id
            || connector.get::<_, String>(1) != work.workstream_id.to_string()
            || connector.get::<_, String>(2) != auth.actor_id
            || connector.get::<_, String>(3) != auth.client_id
            || connector.get::<_, String>(6) != "adapter_observation"
            || !connector.get::<_, bool>(8)
        {
            return Err(PgError::Forbidden);
        }
        if connector.get::<_, String>(4) != auth.epoch {
            return Err(PgError::EpochChanged);
        }
        let connector_version = connector.get::<_, i64>(5).to_string();
        let resource: String = connector.get(7);
        let provider: String = connector.get(9);
        let selected = snapshots::selection(&tx, tenant, project, &query.work_id).await?;
        let mut selection = Value::Null;
        let mut selection_state = "missing";
        if let Some((candidate, candidate_digest, snapshot, owner, fence, selection_version)) =
            selected
        {
            DeliveryRecord::Candidate(candidate.clone())
                .validate()
                .map_err(|_| PgError::SourceDivergence)?;
            if candidate
                .binding
                .digest()
                .map_err(|_| PgError::SourceDivergence)?
                != candidate_digest
                || candidate.binding.tenant_id.as_str() != tenant
                || candidate.binding.project_id.as_str() != project
                || candidate.binding.scope_id.as_str() != "main"
                || candidate.binding.work_id.as_str() != query.work_id
            {
                return Err(PgError::SourceDivergence);
            }
            let current = snapshot == auth.snapshot
                && owner == ownership
                && fence
                && auth::bind_candidate(tenant, project, &set, &candidate.binding).is_ok()
                && candidate.binding.target.resource == resource
                && candidate
                    .binding
                    .source_revision
                    .as_ref()
                    .is_none_or(|r| r.resource == resource);
            selection_state = if current { "current" } else { "stale" };
            selection = json!({"candidate":candidate,"candidate_digest":candidate_digest,
                "selection_version":selection_version.to_string(),"current":current});
        }
        let binding = hash(&json!({"protocol":"awr-delivery-schedule-v1",
            "tenant":tenant,"project":project,"principal":auth.binding,"grants":auth.grant_versions,
            "read_set":set,"connector_id":query.connector_id,"connector_version":connector_version,
            "provider":provider,"resource":resource,
            "selection_state":selection_state,"selection_digest":selection.get("candidate_digest"),"selection_version":selection.get("selection_version")}))?;
        let cursor = query
            .cursor
            .as_ref()
            .map(|raw| {
                let c: Cursor = serde_json::from_str(raw).map_err(|_| PgError::CursorExpired)?;
                if c.binding != binding
                    || identity(&c.after_id).is_err()
                    || identity(&c.upper_id).is_err()
                {
                    return Err(PgError::CursorExpired);
                }
                Ok(c)
            })
            .transpose()?;
        // Both anchors must exist inside the admitted work/connector/resource.
        // Timestamps are fetched from PG, never parsed from an untrusted cursor.
        let (after, upper) = if let Some(c) = cursor {
            let count: i64 = tx
                .query_one(
                    "SELECT count(*) FROM awr_team.delivery_integration_intents
                WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND connector_id=$4
                  AND request_json->'request'->'binding'->'target'->>'resource'=$5 AND id=ANY($6)",
                    &[
                        &tenant,
                        &project,
                        &query.work_id,
                        &query.connector_id,
                        &resource,
                        &vec![c.after_id.clone(), c.upper_id.clone()],
                    ],
                )
                .await?
                .get(0);
            if count != if c.after_id == c.upper_id { 1 } else { 2 } {
                return Err(PgError::CursorExpired);
            }
            (Some(c.after_id), Some(c.upper_id))
        } else {
            let upper = tx
                .query_opt(
                    "SELECT id FROM awr_team.delivery_integration_intents
                WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND connector_id=$4
                  AND request_json->'request'->'binding'->'target'->>'resource'=$5
                ORDER BY created_at DESC,id DESC LIMIT 1",
                    &[
                        &tenant,
                        &project,
                        &query.work_id,
                        &query.connector_id,
                        &resource,
                    ],
                )
                .await?
                .map(|r| r.get::<_, String>(0));
            (None, upper)
        };
        let rows = tx.query("SELECT d.id,d.state,d.request_json->'prepare',d.request_json->'request',c.body_json,
            d.lease_id,d.lease_expires_at::text,d.lease_expires_at>clock_timestamp(),
            d.dispatched_at::text,d.confirmation_fact_id,d.confirmation_current
            FROM awr_team.delivery_integration_intents d LEFT JOIN awr_team.delivery_candidates c
              ON c.tenant_id=d.tenant_id AND c.project_id=d.project_id AND c.work_id=d.work_id
              AND c.binding_digest=d.request_json->'prepare'->>'candidate_digest'
            WHERE d.tenant_id=$1 AND d.project_id=$2 AND d.work_id=$3 AND d.connector_id=$4
              AND d.request_json->'request'->'binding'->'target'->>'resource'=$5
              AND ($6::text IS NULL OR (d.created_at,d.id)>(SELECT created_at,id
                FROM awr_team.delivery_integration_intents WHERE tenant_id=$1 AND project_id=$2 AND id=$6))
              AND (d.created_at,d.id)<=(SELECT created_at,id FROM awr_team.delivery_integration_intents
                WHERE tenant_id=$1 AND project_id=$2 AND id=$7)
            ORDER BY d.created_at,d.id LIMIT $8::integer",
            &[&tenant,&project,&query.work_id,&query.connector_id,&resource,&after,&upper,&(query.limit+1)]).await?;
        let mut result = json!({"protocol":"awr-delivery-schedule-v1","read_only":true,
            "state_basis":"at_read","project_revision":auth.revision.to_string(),"read_set":set,
            "connector":{"connector_id":query.connector_id,"connector_version":connector_version,
                "provider":provider,"resource":resource},"selection_state":selection_state,"selection":selection,
            "integrations":[],"next_cursor":null,"truncated":false,
            "execution_authorized":false,"acceptance_ready":false,"source_synchronized":false});
        let row_count = rows.len();
        let mut items = Vec::new();
        let mut last = None;
        let mut has_more = row_count > query.limit as usize;
        for row in rows.into_iter().take(query.limit as usize) {
            let id: String = row.get(0);
            let prepare: PrepareDeliveryIntegration =
                serde_json::from_value(row.get(2)).map_err(|_| PgError::SourceDivergence)?;
            let request: awr_team::delivery::IntegrationRequest =
                serde_json::from_value(row.get(3)).map_err(|_| PgError::SourceDivergence)?;
            let candidate: DeliveryCandidate = serde_json::from_value(
                row.try_get::<_, Option<Value>>(4)?
                    .ok_or(PgError::SourceDivergence)?,
            )
            .map_err(|_| PgError::SourceDivergence)?;
            DeliveryRecord::Candidate(candidate.clone())
                .validate()
                .map_err(|_| PgError::SourceDivergence)?;
            DeliveryRecord::IntegrationRequest(request.clone())
                .validate()
                .map_err(|_| PgError::SourceDivergence)?;
            auth::bind_candidate(tenant, project, &prepare.read_set, &candidate.binding)
                .map_err(|_| PgError::SourceDivergence)?;
            if prepare.read_set.work_id != query.work_id
                || prepare.connector_id != query.connector_id
                || candidate.binding != request.binding
                || request.request_id.as_str() != id
                || candidate
                    .binding
                    .digest()
                    .map_err(|_| PgError::SourceDivergence)?
                    != prepare.candidate_digest
            {
                return Err(PgError::SourceDivergence);
            }
            let state: String = row.get(1);
            let dispatched_at: Option<String> = row.get(8);
            let query_original = dispatched_at.is_some();
            items.push(json!({"integration_id":id,"state":state,"candidate":candidate,
                "integration_request":request,"original_read_set":prepare.read_set,
                "original_connector_version":prepare.connector_version,"selection_version":prepare.selection_version,
                "candidate_digest":prepare.candidate_digest,"lease_id":row.get::<_,Option<String>>(5),
                "lease_expires_at":row.get::<_,Option<String>>(6),"lease_live":row.get::<_,Option<bool>>(7).unwrap_or(false),
                "dispatched_at":dispatched_at,"confirmation_fact_id":row.get::<_,Option<String>>(9),
                "confirmation_current":row.get::<_,Option<bool>>(10),"query_original":query_original,
                "execution_authorized":false}));
            result["integrations"] = json!(items);
            // Reserve enough space for a continuation and its truncation flag.
            if serde_json::to_vec(&result).map_err(|_| invalid())?.len() + 1024 > MAX_RESPONSE_BYTES
            {
                items.pop();
                result["integrations"] = json!(items);
                has_more = true;
                break;
            }
            last = Some(id);
        }
        if has_more {
            let last = last.ok_or(PgError::ResponseTooLarge)?;
            result["next_cursor"] = json!(encode_cursor(
                &binding,
                &last,
                upper.as_deref().ok_or(PgError::SourceDivergence)?
            )?);
            result["truncated"] = json!(true);
        }
        if serde_json::to_vec(&result).map_err(|_| invalid())?.len() > MAX_RESPONSE_BYTES {
            return Err(PgError::ResponseTooLarge);
        }
        tx.commit().await?;
        Ok(result)
    }
}
