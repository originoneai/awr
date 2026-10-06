use super::*;
use crate::workstream_auth::ReaderAuthority;
use awr_team::Action;
use serde_json::json;
use tokio_postgres::Transaction;

pub(super) struct Connector {
    pub version: i64,
    pub generation: i64,
    pub source: FactSource,
    pub resource: String,
}

pub(super) async fn load(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    set: &DeliveryReadSet,
    id: &str,
) -> PgResult<Connector> {
    identity(id)?;
    let row = tx.query_opt("SELECT work_id,workstream_id,principal_actor_id,principal_client_id,
        coordinator_epoch,version,inspection_generation,fact_source,resource,enabled
        FROM awr_team.delivery_connectors WHERE tenant_id=$1 AND project_id=$2 AND id=$3 FOR UPDATE",
        &[&tenant,&project,&id]).await?.ok_or(PgError::Forbidden)?;
    if row.get::<_, String>(0) != set.work_id
        || row.get::<_, String>(1) != set.workstream_id.to_string()
        || row.get::<_, String>(2) != auth.actor_id
        || row.get::<_, String>(3) != auth.client_id
        || !row.get::<_, bool>(9)
    {
        return Err(PgError::Forbidden);
    }
    if row.get::<_, String>(4) != auth.epoch {
        return Err(PgError::EpochChanged);
    }
    let source = serde_json::from_value(json!(row.get::<_, String>(7)))
        .map_err(|_| PgError::SourceDivergence)?;
    Ok(Connector {
        version: row.get(5),
        generation: row.get(6),
        source,
        resource: row.get(8),
    })
}

impl DeliverySyncStore {
    /// Explicit management authority configures mappings, without granting the principal permissions.
    pub async fn configure_connector(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        request: ConfigureDeliveryConnector,
    ) -> PgResult<Value> {
        bounded(&request)?;
        let m = &request.mapping;
        for value in [
            &m.connector_id,
            &m.principal_actor_id,
            &m.principal_client_id,
        ] {
            identity(value)?;
        }
        text(&m.provider, 128)?;
        text(&m.resource, 1024)?;
        let expected = version(&request.expected_connector_version)?;
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
        let op = "delivery.connector.configure";
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
        let principal = tx.query_opt("SELECT kind FROM awr_team.actors WHERE tenant_id=$1 AND id=$2 AND status='active' FOR SHARE",
            &[&tenant,&m.principal_actor_id]).await?.ok_or(PgError::Forbidden)?.get::<_,String>(0);
        if matches!(m.fact_source, FactSource::AdapterObservation) && principal != "system"
            || matches!(m.fact_source, FactSource::OperatorRecorded)
                && !matches!(principal.as_str(), "human" | "system")
        {
            return Err(PgError::Forbidden);
        }
        let row = tx
            .query_opt(
                "SELECT version,work_id,workstream_id FROM awr_team.delivery_connectors
            WHERE tenant_id=$1 AND project_id=$2 AND id=$3 FOR UPDATE",
                &[&tenant, &project, &m.connector_id],
            )
            .await?;
        if let Some(row) = &row {
            if row.get::<_, String>(1) != set.work_id
                || row.get::<_, String>(2) != set.workstream_id.to_string()
            {
                return Err(PgError::BindingInvalid);
            }
        }
        if row.map(|r| r.get::<_, i64>(0)).unwrap_or(0) != expected {
            return Err(PgError::PreconditionsChanged);
        }
        let next = expected
            .checked_add(1)
            .ok_or(PgError::PreconditionsChanged)?;
        let source = serde_json::to_value(&m.fact_source).map_err(|_| invalid())?;
        let source = source.as_str().ok_or_else(invalid)?;
        tx.execute("INSERT INTO awr_team.delivery_connectors(tenant_id,project_id,id,scope_id,work_id,workstream_id,
            provider,resource,principal_actor_id,principal_client_id,fact_source,version,coordinator_epoch,enabled,configured_by_actor_id)
            VALUES($1,$2,$3,'main',$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
            ON CONFLICT(tenant_id,project_id,id) DO UPDATE SET provider=EXCLUDED.provider,resource=EXCLUDED.resource,
              principal_actor_id=EXCLUDED.principal_actor_id,principal_client_id=EXCLUDED.principal_client_id,
              fact_source=EXCLUDED.fact_source,version=EXCLUDED.version,coordinator_epoch=EXCLUDED.coordinator_epoch,
              enabled=EXCLUDED.enabled,configured_by_actor_id=EXCLUDED.configured_by_actor_id",
            &[&tenant,&project,&m.connector_id,&set.work_id,&set.workstream_id.to_string(),&m.provider,&m.resource,
              &m.principal_actor_id,&m.principal_client_id,&source,&next,&auth.epoch,&m.enabled,&auth.actor_id]).await?;
        let result = auth::finish(&tx,tenant,project,&auth,set,op,&request.request_id,&hash,
            json!({"connector_id":m.connector_id,"connector_version":next.to_string(),"enabled":m.enabled})).await?;
        tx.commit().await?;
        Ok(result)
    }
}
