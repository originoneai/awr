//! Bind terminal advice to the selected version; unsettled effects retain priority.
use super::*;

impl DeliverySyncStore {
    pub(crate) async fn integration_summary_in_tx(
        tx: &Transaction<'_>,
        tenant: &str,
        project: &str,
        auth: &ReaderAuthority,
        work: &str,
        id: Option<&str>,
    ) -> PgResult<Value> {
        let row = tx.query_opt("SELECT i.id,i.state,r.execution_id,
            coalesce(s.source_snapshot_id=$4
              AND s.ownership_version=o.ownership_version
              AND c.body_json->'binding'->>'contract_hash'=wc.contract_hash
              AND i.request_json#>>'{prepare,candidate_digest}'=s.binding_digest
              AND i.request_json#>>'{prepare,selection_version}'=s.selection_version::text
              AND i.request_json#>>'{prepare,read_set,source_snapshot_id}'=$4
              AND i.request_json#>>'{prepare,read_set,coordinator_epoch}'=$5
              AND i.request_json#>>'{prepare,read_set,ownership_version}'=o.ownership_version::text
              AND i.request_json#>>'{prepare,read_set,work_id}'=wc.work_id
              AND i.request_json#>>'{prepare,read_set,workstream_id}'=o.workstream_id
              AND i.request_json#>>'{prepare,read_set,contract_hash}'=wc.contract_hash,false),
            coalesce(f.envelope_json#>>'{record,data,outcome}'='rejected'
              AND jsonb_typeof(f.envelope_json#>'{record,data,result_revision}')='object'
              AND (f.envelope_json#>>'{record,data,binding,target,precondition,kind}'='missing'
                OR f.envelope_json#>'{record,data,result_revision}'<>f.envelope_json#>'{record,data,binding,target,precondition,revision}'),false)
        FROM awr_team.delivery_integration_intents i
        LEFT JOIN awr_team.delivery_selections s ON (s.tenant_id,s.project_id,s.work_id)=(i.tenant_id,i.project_id,i.work_id) AND s.scope_id='main'
        LEFT JOIN awr_team.delivery_candidates c ON (c.tenant_id,c.project_id,c.binding_digest)=(s.tenant_id,s.project_id,s.binding_digest)
        LEFT JOIN awr_team.workstream_snapshot_ownership o ON (o.tenant_id,o.project_id,o.work_id,o.scope_id)=(s.tenant_id,s.project_id,s.work_id,s.scope_id) AND o.snapshot_id=$4
        LEFT JOIN awr_team.work_contracts wc ON (wc.tenant_id,wc.project_id,wc.work_id,wc.scope_id,wc.snapshot_id)=(o.tenant_id,o.project_id,o.work_id,o.scope_id,o.snapshot_id)
        LEFT JOIN awr_team.review_rounds r ON (r.tenant_id,r.project_id,r.work_id)=(i.tenant_id,i.project_id,i.work_id) AND r.id=i.request_json#>>'{prepare,review_round_id}'
        LEFT JOIN awr_team.delivery_facts f ON (f.tenant_id,f.project_id,f.id)=(i.tenant_id,i.project_id,i.confirmation_fact_id)
        WHERE i.tenant_id=$1 AND i.project_id=$2 AND i.work_id=$3 AND ($6::text IS NULL OR i.id=$6)
        ORDER BY (i.state IN ('dispatched','unknown')) DESC,i.created_at DESC,i.id DESC LIMIT 1",
            &[&tenant,&project,&work,&auth.snapshot,&auth.epoch,&id]).await?;
        Ok(row.map_or(Value::Null, |r| {
            json!({"id":r.get::<_,String>(0),
            "state":r.get::<_,String>(1),"reviewed_execution_id":r.get::<_,Option<String>>(2),
            "current_binding":r.get::<_,bool>(3),"target_changed":r.get::<_,bool>(4)})
        }))
    }
}
