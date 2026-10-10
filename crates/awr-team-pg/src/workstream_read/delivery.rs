//! Neutral facts and outcomes stay in the caller's authenticated read snapshot.
use super::*;
use crate::DeliverySyncStore;

pub(super) async fn read(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    q: &WorkstreamQuery,
    work: &str,
) -> PgResult<Value> {
    let result = match q.op.as_str() {
        "delivery.neutral.inspect" => {
            DeliverySyncStore::inspect_in_tx(tx, tenant, project, auth, work).await?
        }
        "delivery.submission.describe" => {
            DeliverySyncStore::describe_submission_in_tx(tx, tenant, project, auth, work).await?
        }
        "delivery.source.status" => {
            DeliverySyncStore::source_publication_status_in_tx(tx, tenant, project, auth, work)
                .await?
        }
        "delivery.integration.inspect" => {
            let id = q.request_id.as_deref().ok_or(PgError::Forbidden)?;
            DeliverySyncStore::inspect_integration_in_tx(tx, tenant, project, auth, work, id)
                .await?
        }
        "delivery.neutral.outcome" => {
            let _ = work_binding(tx, tenant, project, auth, work).await?;
            let request_id = q.request_id.as_deref().ok_or(PgError::Forbidden)?;
            let row = tx.query_opt("SELECT result_json FROM awr_team.delivery_sync_requests
                WHERE tenant_id=$1 AND project_id=$2 AND actor_id=$3 AND client_id=$4 AND request_id=$5",
                &[&tenant,&project,&auth.actor_id,&auth.client_id,&request_id]).await?;
            let receipt = row.map(|row| row.get::<_, Value>(0));
            if receipt.as_ref().is_some_and(|r| r["work_id"] != work) {
                return Err(PgError::Forbidden);
            }
            json!({"request_id":request_id,"work_id":work,
                "outcome":if receipt.is_some(){"committed"}else{"unknown"},
                "state_basis":"at_commit","receipt":receipt,"execution_authorized":false,
                "guidance":{"when":"A neutral command response is missing or historical",
                    "because":"A receipt records one commit; absence does not exclude a concurrent commit",
                    "action":"Query current neutral facts and source status; keep the original request identity for recovery",
                    "recheck_on":"Receipt, candidate, source or authority changes"}})
        }
        _ => return Err(PgError::Unsupported("neutral delivery query".into())),
    };
    if serde_json::to_vec(&result)
        .map_err(|_| PgError::SourceDivergence)?
        .len()
        > 262144
    {
        return Err(PgError::ResponseTooLarge);
    }
    Ok(result)
}
