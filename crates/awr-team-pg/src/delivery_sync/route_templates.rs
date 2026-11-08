//! Scoped configuration references; registration never proves worker availability.
use super::*;
use crate::workstream_auth::ReaderAuthority;
use awr_team::Action;
use serde_json::json;
use tokio_postgres::Transaction;

impl DeliverySyncStore {
    pub async fn configure_route_template(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        request: ConfigureDeliveryRouteTemplate,
    ) -> PgResult<Value> {
        bounded(&request)?;
        identity(&request.template_id)?;
        identity(&request.source_connector_id)?;
        let expected = version(&request.expected_template_version)?;
        let source_version = version(&request.source_connector_version)?;
        if source_version == 0 {
            return Err(invalid());
        }
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let set = &request.read_set;
        let auth = auth::admit(
            &tx,
            tenant,
            project,
            bearer,
            set,
            Action::AccessManageProject,
            None,
        )
        .await?;
        let op = "delivery.route.configure";
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
        let previous = tx
            .query_opt(
                "SELECT version,workstream_id,source_connector_id,source_connector_version
             FROM awr_team.delivery_route_templates
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3 FOR UPDATE",
                &[&tenant, &project, &request.template_id],
            )
            .await?;
        if previous.as_ref().map_or(0, |r| r.get::<_, i64>(0)) != expected {
            return Err(PgError::PreconditionsChanged);
        }
        if previous
            .as_ref()
            .is_some_and(|r| r.get::<_, String>(1) != set.workstream_id.to_string())
        {
            return Err(PgError::BindingInvalid);
        }
        if request.enabled {
            let source = tx
                .query_opt(
                    "SELECT c.work_id,c.workstream_id,c.version,c.coordinator_epoch,c.enabled,
                        c.fact_source,a.kind,a.status
                 FROM awr_team.delivery_connectors c JOIN awr_team.actors a
                   ON a.tenant_id=c.tenant_id AND a.id=c.principal_actor_id
                 WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.id=$3 FOR SHARE OF c,a",
                    &[&tenant, &project, &request.source_connector_id],
                )
                .await?
                .ok_or(PgError::Forbidden)?;
            if source.get::<_, String>(0) != set.work_id
                || source.get::<_, String>(1) != set.workstream_id.to_string()
            {
                return Err(PgError::Forbidden);
            }
            if source.get::<_, i64>(2) != source_version
                || source.get::<_, String>(3) != auth.epoch
                || !source.get::<_, bool>(4)
                || source.get::<_, String>(5) != "adapter_observation"
                || source.get::<_, String>(6) != "system"
                || source.get::<_, String>(7) != "active"
            {
                return Err(PgError::PreconditionsChanged);
            }
        } else {
            // Revocation remains possible after a connector changes or is disabled.
            let previous = previous.as_ref().ok_or(PgError::PreconditionsChanged)?;
            if previous.get::<_, String>(2) != request.source_connector_id
                || previous.get::<_, i64>(3) != source_version
            {
                return Err(PgError::PreconditionsChanged);
            }
        }
        let next = expected
            .checked_add(1)
            .ok_or(PgError::PreconditionsChanged)?;
        tx.execute(
            "INSERT INTO awr_team.delivery_route_templates(tenant_id,project_id,id,workstream_id,
                 source_connector_id,source_connector_version,version,coordinator_epoch,enabled,configured_by_actor_id)
             VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
             ON CONFLICT(tenant_id,project_id,id) DO UPDATE SET
                 source_connector_id=EXCLUDED.source_connector_id,
                 source_connector_version=EXCLUDED.source_connector_version,
                 version=EXCLUDED.version,coordinator_epoch=EXCLUDED.coordinator_epoch,
                 enabled=EXCLUDED.enabled,configured_by_actor_id=EXCLUDED.configured_by_actor_id",
            &[&tenant,&project,&request.template_id,&set.workstream_id.to_string(),
              &request.source_connector_id,&source_version,&next,&auth.epoch,&request.enabled,&auth.actor_id],
        ).await?;
        let result = auth::finish(
            &tx,
            tenant,
            project,
            &auth,
            set,
            op,
            &request.request_id,
            &request_hash,
            json!({"template_id":request.template_id,"template_version":next.to_string(),
                "source_connector_version":source_version.to_string(),"enabled":request.enabled,
                "existing_task_mappings_changed":false,"grants_created":false}),
        )
        .await?;
        tx.commit().await?;
        Ok(result)
    }

    pub(crate) async fn route_templates_in_tx(
        tx: &Transaction<'_>,
        tenant: &str,
        project: &str,
        auth: &ReaderAuthority,
        work: &str,
        after: &str,
        limit: i64,
    ) -> PgResult<Vec<Value>> {
        let (binding, _) =
            crate::workstream_read::work_binding(tx, tenant, project, auth, work).await?;
        crate::workstream_auth::authorize_query(
            auth,
            Some(binding.workstream_id),
            Some(work),
            "delivery.routes",
        )?;
        let rows = tx.query(
            "SELECT t.id,t.version,c.version,c.provider,c.resource,c.source_locator_contract,c.source_locator_url,
                    c.source_locator_connector_version
             FROM awr_team.delivery_route_templates t JOIN awr_team.delivery_connectors c
               ON c.tenant_id=t.tenant_id AND c.project_id=t.project_id AND c.id=t.source_connector_id
             JOIN awr_team.actors a ON a.tenant_id=c.tenant_id AND a.id=c.principal_actor_id
             JOIN awr_team.workstream_snapshot_ownership o
               ON o.tenant_id=c.tenant_id AND o.project_id=c.project_id AND o.work_id=c.work_id
               AND o.scope_id='main' AND o.snapshot_id=$7 AND o.workstream_id=t.workstream_id
             WHERE t.tenant_id=$1 AND t.project_id=$2 AND t.workstream_id=$3 AND t.id>$4
               AND t.enabled AND c.enabled AND t.coordinator_epoch=$5 AND c.coordinator_epoch=$5
               AND c.workstream_id=t.workstream_id AND c.version=t.source_connector_version
               AND c.fact_source='adapter_observation' AND a.kind='system' AND a.status='active'
             ORDER BY t.id LIMIT $6",
            &[&tenant,&project,&binding.workstream_id.to_string(),&after,&auth.epoch,&limit,&auth.snapshot],
        ).await?;
        Ok(rows.into_iter().map(|r| {
            let contract: Option<String> = r.get(5);
            let url: Option<String> = r.get(6);
            let locator = contract.zip(url).filter(|(contract, url)| {
                r.get::<_, Option<i64>>(7) == Some(r.get::<_, i64>(2))
                    && contract == connectors::GIT_SOURCE_CONTRACT
                    && connectors::safe_source_url(url)
            });
            json!({"template_id":r.get::<_,String>(0),"template_version":r.get::<_,i64>(1).to_string(),
                "source_connector_version":r.get::<_,i64>(2).to_string(),"provider":r.get::<_,String>(3),
                "resource":r.get::<_,String>(4),"source_locator":locator.map(|(contract,url)|
                    json!({"contract":contract,"url":url})),
                "basis":"current_enabled_template_and_connector_registration"})
        }).collect())
    }
}
