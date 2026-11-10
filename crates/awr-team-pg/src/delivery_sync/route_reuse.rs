//! Selecting a configured route grants neither access nor execution authority.
use super::*;
use awr_team::Action;
use serde_json::json;

impl DeliverySyncStore {
    pub async fn reuse_route_template(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        request: ReuseDeliveryRouteTemplate,
    ) -> PgResult<Value> {
        bounded(&request)?;
        identity(&request.template_id)?;
        identity(&request.connector_id)?;
        let template_version = version(&request.template_version)?;
        let source_version = version(&request.source_connector_version)?;
        if template_version == 0 || source_version == 0 {
            return Err(invalid());
        }
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let set = &request.read_set;
        let auth = auth::admit(&tx, tenant, project, bearer, set, Action::WorkAssign, None).await?;
        let op = "delivery.route.reuse";
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
        crate::responsibility::lock_transition_keys(&tx, tenant, project, &set.work_id, &[])
            .await?;
        crate::lock_works_sorted(&tx, tenant, project, "main", &[set.work_id.clone()]).await?;
        crate::workstream_command::task_intake::require_enabled(
            &tx,
            tenant,
            project,
            &auth,
            &set.work_id,
        )
        .await?;
        use super::route_state::TaskRouteState;
        let state = Self::task_route_state_in_tx(
            &tx,
            tenant,
            project,
            &set.work_id,
            &set.workstream_id.to_string(),
            &auth.epoch,
        )
        .await?;
        if state != TaskRouteState::Missing {
            return Err(PgError::DeliveryRouteReuseBlocked {
                reason: if state == TaskRouteState::Entered {
                    "work_entered"
                } else {
                    "existing_mapping"
                },
            });
        }
        // A conflicting identifier may belong to another work; disclose no details about it.
        let occupied: bool = tx
            .query_one(
                "SELECT EXISTS(SELECT 1 FROM awr_team.delivery_connectors
            WHERE tenant_id=$1 AND project_id=$2 AND id=$3)",
                &[&tenant, &project, &request.connector_id],
            )
            .await?
            .get(0);
        if occupied {
            return Err(PgError::PreconditionsChanged);
        }
        let row = tx.query_opt("SELECT t.workstream_id,t.version,t.enabled,t.coordinator_epoch,
            c.version,c.enabled,c.coordinator_epoch,c.fact_source,c.workstream_id,
            a.kind,a.status,c.provider,c.resource,c.principal_actor_id,c.principal_client_id,
            c.source_locator_contract,c.source_locator_url,c.source_locator_connector_version,
            EXISTS(SELECT 1 FROM awr_team.workstream_snapshot_ownership o
                WHERE o.tenant_id=c.tenant_id AND o.project_id=c.project_id AND o.work_id=c.work_id
                    AND o.scope_id='main' AND o.snapshot_id=$4 AND o.workstream_id=t.workstream_id)
            FROM awr_team.delivery_route_templates t
            JOIN awr_team.delivery_connectors c ON (c.tenant_id,c.project_id,c.id)=(t.tenant_id,t.project_id,t.source_connector_id)
            JOIN awr_team.actors a ON (a.tenant_id,a.id)=(c.tenant_id,c.principal_actor_id)
            WHERE t.tenant_id=$1 AND t.project_id=$2 AND t.id=$3
                AND c.version=t.source_connector_version FOR SHARE OF t,c,a",
            &[&tenant,&project,&request.template_id,&auth.snapshot]).await?
            .ok_or(PgError::PreconditionsChanged)?;
        if row.get::<_, String>(0) != set.workstream_id.to_string()
            || row.get::<_, String>(8) != set.workstream_id.to_string()
        {
            return Err(PgError::Forbidden);
        }
        if row.get::<_, i64>(1) != template_version
            || !row.get::<_, bool>(2)
            || row.get::<_, String>(3) != auth.epoch
            || row.get::<_, i64>(4) != source_version
            || !row.get::<_, bool>(5)
            || row.get::<_, String>(6) != auth.epoch
            || row.get::<_, String>(7) != "adapter_observation"
            || row.get::<_, String>(9) != "system"
            || row.get::<_, String>(10) != "active"
            || !row.get::<_, bool>(18)
        {
            return Err(PgError::PreconditionsChanged);
        }
        // Registration may follow implementation/review, but cannot reinterpret
        // an existing selection or attach its delivery to another repository.
        if let Some((candidate, digest, snapshot, ownership, fence_current, _)) =
            snapshots::selection(&tx, tenant, project, &set.work_id).await?
        {
            auth::bind_candidate(tenant, project, set, &candidate.binding)?;
            let resource: String = row.get(12);
            if candidate.binding.target.resource != resource
                || candidate
                    .binding
                    .source_revision
                    .as_ref()
                    .is_some_and(|r| r.resource != resource)
            {
                return Err(PgError::DeliveryRouteReuseBlocked {
                    reason: "candidate_resource",
                });
            }
            if snapshot != auth.snapshot
                || ownership != version(&set.ownership_version)?
                || !fence_current
                || candidate.binding.digest().map_err(|_| invalid())? != digest
            {
                return Err(PgError::InactiveCandidate);
            }
        }
        let locator_contract: Option<String> = row.get(15);
        let locator_url: Option<String> = row.get(16);
        let locator = locator_contract.zip(locator_url).filter(|(contract, url)| {
            row.get::<_, Option<i64>>(17) == Some(source_version)
                && contract == connectors::GIT_SOURCE_CONTRACT
                && connectors::safe_source_url(url)
        });
        let (contract, url) = locator.map_or((None, None), |(c, u)| (Some(c), Some(u)));
        let locator_version = contract.as_ref().map(|_| 1_i64);
        tx.execute("INSERT INTO awr_team.delivery_connectors(tenant_id,project_id,id,scope_id,work_id,workstream_id,
            provider,resource,principal_actor_id,principal_client_id,fact_source,version,coordinator_epoch,enabled,
            configured_by_actor_id,source_locator_contract,source_locator_url,source_locator_connector_version)
            VALUES($1,$2,$3,'main',$4,$5,$6,$7,$8,$9,'adapter_observation',1,$10,true,$11,$12,$13,$14)",
            &[&tenant,&project,&request.connector_id,&set.work_id,&set.workstream_id.to_string(),
              &row.get::<_,String>(11),&row.get::<_,String>(12),&row.get::<_,String>(13),&row.get::<_,String>(14),
              &auth.epoch,&auth.actor_id,&contract,&url,&locator_version]).await?;
        let result = auth::finish(&tx,tenant,project,&auth,set,op,&request.request_id,&request_hash,
            json!({"template_id":request.template_id,"template_version":request.template_version,
                "source_connector_version":request.source_connector_version,"connector_id":request.connector_id,
                "connector_version":"1","enabled":true,"existing_task_mappings_changed":false,"grants_created":false})).await?;
        tx.commit().await?;
        Ok(result)
    }
}
