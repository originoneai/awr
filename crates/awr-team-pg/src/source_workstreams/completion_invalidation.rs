//! Reopen current selections while retaining verified, explicitly fixed inputs.
use super::*;
use awr_core::DeliveryVersionPolicy;
use awr_team::DependencyAcceptanceMode;
use std::collections::VecDeque;

pub(super) async fn apply(
    projection: &SourceProjection,
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    snapshot: &str,
) -> PgResult<Vec<Value>> {
    // The source activation already holds the project barrier. Its new
    // projection is installed, but the active snapshot has not switched yet.
    let rows = tx
        .query(
            "SELECT r.id,r.work_id,r.contract_hash FROM awr_team.completion_receipts r
        JOIN awr_team.work_runtime w ON w.tenant_id=r.tenant_id AND w.project_id=r.project_id
          AND w.scope_id=r.scope_id AND w.work_id=r.work_id AND w.selected_completion_id=r.id
        WHERE r.tenant_id=$1 AND r.project_id=$2 AND r.scope_id='main' AND w.state='completed'",
            &[&tenant, &project],
        )
        .await?;
    let selected: BTreeMap<String, (String, String)> = rows
        .into_iter()
        .map(|r| (r.get(1), (r.get(0), r.get(2))))
        .collect();
    let contracts: BTreeMap<_, _> = projection
        .contracts
        .iter()
        .map(|c| (c.work_id.as_str(), c))
        .collect();
    let mut stale = BTreeSet::new();
    for (work, (receipt, hash)) in &selected {
        if projection.hashes.get(work) != Some(hash) {
            stale.insert(receipt.clone());
        }
    }
    let mut dependents: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let links = tx
        .query(
            "SELECT d.completion_id,d.predecessor_work_id,d.predecessor_completion_id,
            w.work_id,p.state,p.selected_completion_id
        FROM awr_team.completion_dependencies d
        JOIN awr_team.work_runtime w ON w.tenant_id=d.tenant_id AND w.project_id=d.project_id
          AND w.scope_id='main' AND w.state='completed' AND w.selected_completion_id=d.completion_id
        LEFT JOIN awr_team.work_runtime p ON p.tenant_id=d.tenant_id AND p.project_id=d.project_id
          AND p.scope_id='main' AND p.work_id=d.predecessor_work_id
        WHERE d.tenant_id=$1 AND d.project_id=$2",
            &[&tenant, &project],
        )
        .await?;
    for row in links {
        let receipt: String = row.get(0);
        let upstream: String = row.get(1);
        let original: String = row.get(2);
        let consumer: String = row.get(3);
        let fixed = contracts.get(consumer.as_str()).and_then(|c| {
            match c.dependency_acceptance.get(&upstream) {
                Some(DependencyAcceptanceMode::CrossWorkstream(policy))
                    if policy.version_policy == DeliveryVersionPolicy::FixedDelivery =>
                {
                    Some(*policy)
                }
                _ => None,
            }
        });
        if let Some(policy) = fixed {
            let actual = crate::cross_workstream_adoption::adopted_receipt(
                tx, tenant, project, snapshot, &consumer, &upstream, policy,
            )
            .await?;
            if actual.as_deref() != Some(original.as_str()) {
                stale.insert(receipt);
            }
            // Only actual verified history cuts the current-selection chain.
            // The consumer's own changed hash is still an independent seed.
            continue;
        }
        if row.get::<_, Option<String>>(4).as_deref() != Some("completed")
            || row.get::<_, Option<String>>(5).as_deref() != Some(original.as_str())
        {
            stale.insert(receipt.clone());
        }
        dependents.entry(original).or_default().push(receipt);
    }
    // Visit each propagation edge once, without repeatedly walking a long
    // chain or invalidating the whole project. History is never deleted.
    let mut pending: VecDeque<_> = stale.iter().cloned().collect();
    while let Some(receipt) = pending.pop_front() {
        for consumer in dependents.get(&receipt).into_iter().flatten() {
            if stale.insert(consumer.clone()) {
                pending.push_back(consumer.clone());
            }
        }
    }
    let mut result = Vec::new();
    for (work, (receipt, _)) in selected {
        if !stale.contains(&receipt) {
            continue;
        }
        let version: i64 = tx.query_one("UPDATE awr_team.work_runtime SET state='unclaimed',selected_completion_id=NULL,work_version=work_version+1
            WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3
              AND state='completed' AND selected_completion_id=$4 RETURNING work_version",
            &[&tenant,&project,&work,&receipt]).await?.get(0);
        result.push(
            serde_json::json!({"work_id":work,"previous_completion_id":receipt,
            "work_version":version.to_string(),"reason":"contract_or_predecessor_changed"}),
        );
    }
    Ok(result)
}
