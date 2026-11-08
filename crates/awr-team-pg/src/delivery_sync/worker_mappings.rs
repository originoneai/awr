//! Bounded authenticated registrations for an operator-configured repository worker.
use super::*;
use awr_core::{Id, WorkstreamAction};
use awr_team::Action;
use serde_json::json;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryWorkerMappingsQuery {
    pub workstream_id: Id,
    pub provider: String,
    pub resource: String,
    pub require_integration: bool,
    pub cursor: Option<String>,
    pub limit: i32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    binding: String,
    after: String,
    upper: String,
}

impl DeliverySyncStore {
    pub async fn worker_mappings(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        query: DeliveryWorkerMappingsQuery,
    ) -> PgResult<Value> {
        bounded(&query)?;
        text(&query.provider, 128)?;
        text(&query.resource, 1024)?;
        if !(1..=32).contains(&query.limit) || query.cursor.as_ref().is_some_and(|c| c.len() > 4096)
        {
            return Err(invalid());
        }
        crate::retry_rolled_back(|| self.worker_mappings_once(tenant, project, bearer, &query))
            .await
    }

    async fn worker_mappings_once(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        query: &DeliveryWorkerMappingsQuery,
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
        auth.access
            .authorize(&auth.catalog, query.workstream_id, WorkstreamAction::Write)?;
        for action in [Action::WorkRead, Action::DeliverySubmitAndRequestReview]
            .into_iter()
            .chain(
                query
                    .require_integration
                    .then_some(Action::DeliveryFinalize),
            )
        {
            crate::workstream_auth::authorize_domain_action(
                &auth,
                action,
                Some(query.workstream_id),
                None,
            )?;
        }
        let binding = crate::digest_of(
            &json!({"subject":auth.binding,"grants":auth.grant_versions,
            "snapshot":auth.snapshot,"epoch":auth.epoch,"workstream":query.workstream_id,
            "provider":query.provider,"resource":query.resource,"integration":query.require_integration}),
        );
        let cursor: Option<Cursor> = query
            .cursor
            .as_ref()
            .map(|s| serde_json::from_str(s).map_err(|_| PgError::CursorExpired))
            .transpose()?;
        if cursor
            .as_ref()
            .is_some_and(|c| c.binding != binding || c.after.is_empty() || c.upper < c.after)
        {
            return Err(PgError::CursorExpired);
        }
        let after = cursor.as_ref().map_or("", |c| c.after.as_str());
        let upper = cursor.as_ref().map_or("", |c| c.upper.as_str());
        let rows = tx
            .query(
                "SELECT c.id,c.work_id,c.version,
                max(c.id) OVER () FROM awr_team.delivery_connectors c
            JOIN awr_team.workstream_snapshot_ownership o
                ON (o.tenant_id,o.project_id,o.work_id)=(c.tenant_id,c.project_id,c.work_id)
                    AND o.scope_id='main' AND o.snapshot_id=$3 AND o.workstream_id=$4
            WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.workstream_id=$4
                AND c.principal_actor_id=$5 AND c.principal_client_id=$6
                AND c.enabled AND c.coordinator_epoch=$7 AND c.fact_source='adapter_observation'
                AND c.provider=$8 AND c.resource=$9 AND c.id>$10
                AND ($11='' OR c.id<=$11)
            ORDER BY c.id LIMIT $12",
                &[
                    &tenant,
                    &project,
                    &auth.snapshot,
                    &query.workstream_id.to_string(),
                    &auth.actor_id,
                    &auth.client_id,
                    &auth.epoch,
                    &query.provider,
                    &query.resource,
                    &after,
                    &upper,
                    &(i64::from(query.limit) + 1),
                ],
            )
            .await?;
        let more = rows.len() > query.limit as usize;
        let upper = if upper.is_empty() {
            rows.first().map(|r| r.get::<_, String>(3))
        } else {
            Some(upper.into())
        };
        let items: Vec<Value> = rows
            .iter()
            .take(query.limit as usize)
            .map(|r| {
                json!({
            "connector_id":r.get::<_,String>(0),"work_id":r.get::<_,String>(1),
            "connector_version":r.get::<_,i64>(2).to_string()})
            })
            .collect();
        let next = if more {
            Some(
                serde_json::to_string(&Cursor {
                    binding,
                    after: items.last().unwrap()["connector_id"]
                        .as_str()
                        .unwrap()
                        .into(),
                    upper: upper.ok_or(PgError::SourceDivergence)?,
                })
                .map_err(|_| invalid())?,
            )
        } else {
            None
        };
        let result = json!({"protocol":"awr-delivery-worker-mappings-v1","workstream_id":query.workstream_id,
            "provider":query.provider,"resource":query.resource,"items":items,"next_cursor":next,
            "registration_only":true,"execution_authorized":false,"grants_created":false});
        if serde_json::to_vec(&result).map_err(|_| invalid())?.len() > 262144 {
            return Err(PgError::ResponseTooLarge);
        }
        tx.commit().await?;
        Ok(result)
    }
}
