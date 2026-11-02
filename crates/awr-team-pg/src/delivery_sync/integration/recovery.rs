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

    /// Whether completing the work must wait for a confirmed integration: the project declared a
    /// delivery chain for it (the same test that makes a candidate mandatory for review), a
    /// candidate is selected, and its completion policy can be integrated at all. Report-only
    /// work and ordinary human confirmation, which cannot prepare an integration, complete as
    /// before.
    pub(crate) async fn delivers_into_repository_in_tx(
        tx: &Transaction<'_>,
        tenant: &str,
        project: &str,
        work: &str,
        policy: &str,
    ) -> PgResult<bool> {
        if !eligibility::supports_integration(policy)
            || !crate::delivery_sync::review_submission::configured(tx, tenant, project, work)
                .await?
        {
            return Ok(false);
        }
        Ok(tx
            .query_one(
                "SELECT EXISTS(SELECT 1 FROM awr_team.delivery_selections
                    WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3)",
                &[&tenant, &project, &work],
            )
            .await?
            .get(0))
    }

    /// A work that delivers a selected candidate into a repository is complete only after the
    /// integration of exactly that candidate was confirmed. An integration belongs to the
    /// selection it was prepared for: same candidate digest, selection version, source snapshot
    /// and ownership of the selection, and the candidate's own contract and workstream. Integrations
    /// of earlier versions are history and do not count, and a confirmation counts only if it was
    /// current when it was recorded. The refusal names the state that is missing.
    pub(crate) async fn require_confirmed_integration_in_tx(
        tx: &Transaction<'_>,
        tenant: &str,
        project: &str,
        work: &str,
    ) -> PgResult<()> {
        // Most relevant first (unsettled attempts, then the newest): the first intent of this
        // selection explains the refusal unless a later one is the confirmation. A rejected
        // attempt "saw another target" under the same rule as `integration_summary_in_tx`.
        let rows = tx
            .query(
                "SELECT i.state,i.confirmation_current,
                    coalesce(f.envelope_json#>>'{record,data,outcome}'='rejected'
                      AND jsonb_typeof(f.envelope_json#>'{record,data,result_revision}')='object'
                      AND (f.envelope_json#>>'{record,data,binding,target,precondition,kind}'='missing'
                        OR f.envelope_json#>'{record,data,result_revision}'<>f.envelope_json#>'{record,data,binding,target,precondition,revision}'),false)
                 FROM awr_team.delivery_integration_intents i
                 JOIN awr_team.delivery_selections s ON (s.tenant_id,s.project_id,s.work_id)=(i.tenant_id,i.project_id,i.work_id) AND s.scope_id='main'
                 JOIN awr_team.delivery_candidates c ON (c.tenant_id,c.project_id,c.binding_digest)=(s.tenant_id,s.project_id,s.binding_digest)
                 LEFT JOIN awr_team.delivery_facts f ON (f.tenant_id,f.project_id,f.id)=(i.tenant_id,i.project_id,i.confirmation_fact_id)
                 WHERE i.tenant_id=$1 AND i.project_id=$2 AND i.work_id=$3
                   AND i.request_json#>>'{prepare,candidate_digest}'=s.binding_digest
                   AND i.request_json#>>'{prepare,selection_version}'=s.selection_version::text
                   AND i.request_json#>>'{prepare,read_set,source_snapshot_id}'=s.source_snapshot_id
                   AND i.request_json#>>'{prepare,read_set,ownership_version}'=s.ownership_version::text
                   AND i.request_json#>>'{prepare,read_set,work_id}'=s.work_id
                   AND i.request_json#>>'{prepare,read_set,workstream_id}'=c.body_json->'binding'->>'workstream_id'
                   AND i.request_json#>>'{prepare,read_set,contract_hash}'=c.body_json->'binding'->>'contract_hash'
                 ORDER BY (i.state IN ('dispatched','unknown')) DESC,i.created_at DESC,i.id DESC",
                &[&tenant, &project, &work],
            )
            .await?;
        let mut explained: Option<&'static str> = None;
        for row in &rows {
            let state: String = row.get(0);
            let confirmation_current = row.get::<_, Option<bool>>(1).unwrap_or(false);
            if state == "confirmed" && confirmation_current {
                return Ok(());
            }
            explained.get_or_insert(match state.as_str() {
                "prepared" | "leased" => "integration_prepared",
                "dispatched" => "integration_dispatched",
                "unknown" => "integration_unknown",
                "rejected" if row.get::<_, bool>(2) => "integration_target_changed",
                "rejected" => "integration_rejected",
                "confirmed" => "integration_confirmation_not_current",
                _ => "integration_not_prepared",
            });
        }
        Err(PgError::IntegrationNotConfirmed {
            reason: explained.unwrap_or("integration_not_prepared"),
        })
    }
}
