//! Governance of a Team snapshot: the admission policy a source declares and what it says of each
//! governed work. The rows describe governance and never decide anything; a decision is a separate,
//! attributed record. A work the declaration does not list owes a definition, so work added to a
//! governed snapshot after the fact can never escape governance by being absent from it.
use crate::{PgError, PgResult};
use awr_core::{
    AdmissionBinding, AdmissionDecision, AdmissionPolicy, AdmissionStatus, DefinitionSummary, Id,
    RecordedAdmission,
};
use awr_team::{
    GovernanceCandidate, WorkContract, WorkGovernance, admission_status, binding_fingerprint,
};
use serde_json::Value;
use std::collections::BTreeMap;
use tokio_postgres::Transaction;

/// What governs one work in one snapshot.
pub(crate) struct Governed {
    pub policy: AdmissionPolicy,
    pub work: WorkGovernance,
    pub governance_hash: String,
}

/// Writes the declaration of a snapshot; called once, with the snapshot's contracts.
pub(crate) async fn install(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    snapshot: &str,
    candidate: &GovernanceCandidate,
    contracts: &[WorkContract],
) -> PgResult<()> {
    let hash = candidate
        .hash()
        .map_err(|e| PgError::Protocol(e.to_string()))?;
    let policy =
        serde_json::to_value(&candidate.policy).map_err(|e| PgError::Protocol(e.to_string()))?;
    tx.execute(
        "INSERT INTO awr_team.snapshot_governance(tenant_id,project_id,snapshot_id,governance_hash,policy_json)
        VALUES($1,$2,$3,$4,$5)",
        &[&tenant, &project, &snapshot, &hash, &policy],
    )
    .await?;
    let ids: BTreeMap<&str, &str> = contracts
        .iter()
        .map(|c| (c.external_key.as_str(), c.work_id.as_str()))
        .collect();
    for work in &candidate.works {
        let id = ids
            .get(work.external_key.as_str())
            .ok_or_else(|| PgError::Protocol("governance names a work of no contract".into()))?;
        let value = serde_json::to_value(work).map_err(|e| PgError::Protocol(e.to_string()))?;
        tx.execute(
            "INSERT INTO awr_team.work_governance(tenant_id,project_id,snapshot_id,work_id,external_key,governance_json)
            VALUES($1,$2,$3,$4,$5,$6)",
            &[&tenant, &project, &snapshot, id, &work.external_key, &value],
        )
        .await?;
    }
    Ok(())
}

/// The governance of `work` in `snapshot`, or none when the snapshot declares none. Stored
/// governance that cannot be read is a divergence, never a pass.
pub(crate) async fn load(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    snapshot: &str,
    work: &str,
) -> PgResult<Option<Governed>> {
    let Some(header) = tx
        .query_opt(
            "SELECT governance_hash, policy_json FROM awr_team.snapshot_governance
            WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3",
            &[&tenant, &project, &snapshot],
        )
        .await?
    else {
        return Ok(None);
    };
    let governance_hash: String = header.get(0);
    let policy_json: Value = header.get(1);
    let policy: AdmissionPolicy =
        serde_json::from_value(policy_json).map_err(|_| PgError::SourceDivergence)?;
    policy.validate().map_err(|_| PgError::SourceDivergence)?;
    let declared = tx
        .query_opt(
            "SELECT governance_json FROM awr_team.work_governance
            WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND work_id=$4",
            &[&tenant, &project, &snapshot, &work],
        )
        .await?;
    let work = match declared {
        Some(row) => {
            let value: Value = row.get(0);
            serde_json::from_value(value).map_err(|_| PgError::SourceDivergence)?
        }
        None => {
            let key: String = tx
                .query_one(
                    "SELECT external_key FROM awr_team.work_items
                    WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                    &[&tenant, &project, &work],
                )
                .await?
                .get(0);
            WorkGovernance {
                external_key: key,
                tags: vec![],
                definition: None,
                summary: DefinitionSummary {
                    declared: false,
                    intent_adopted: false,
                    gaps: vec![],
                },
            }
        }
    };
    Ok(Some(Governed {
        policy,
        work,
        governance_hash,
    }))
}

/// The latest decision recorded for a work, as the Core evaluator reads it.
async fn latest(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work: &str,
) -> PgResult<Option<RecordedAdmission>> {
    let Some(row) = tx
        .query_opt(
            "SELECT id, decision, binding_json, reason FROM awr_team.work_admissions
            WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3
            ORDER BY created_at DESC, id DESC LIMIT 1",
            &[&tenant, &project, &work],
        )
        .await?
    else {
        return Ok(None);
    };
    let id: String = row.get(0);
    let decision: String = row.get(1);
    let binding: Value = row.get(2);
    let decision: AdmissionDecision =
        serde_json::from_value(Value::String(decision)).map_err(|_| PgError::SourceDivergence)?;
    Ok(Some(RecordedAdmission {
        event: id.parse::<Id>().map_err(|_| PgError::SourceDivergence)?,
        decision,
        bound: serde_json::from_value::<AdmissionBinding>(binding)
            .map_err(|_| PgError::SourceDivergence)?,
        reason: row.get(3),
        proposer_session: None,
        decider_session: None,
    }))
}

/// The admission state of `work` in `snapshot`, with the binding a decider must have read. None
/// when the snapshot declares no governance.
pub(crate) async fn status(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    snapshot: &str,
    work: &str,
) -> PgResult<Option<(AdmissionStatus, AdmissionBinding, Governed)>> {
    let Some(governed) = load(tx, tenant, project, snapshot, work).await? else {
        return Ok(None);
    };
    let contract_hash: String = tx
        .query_opt(
            "SELECT contract_hash FROM awr_team.work_contracts
            WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND scope_id='main' AND work_id=$4",
            &[&tenant, &project, &snapshot, &work],
        )
        .await?
        .ok_or(PgError::SourceDivergence)?
        .get(0);
    let completed: bool = tx
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM awr_team.work_runtime
            WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3
              AND (state='completed' OR selected_completion_id IS NOT NULL))",
            &[&tenant, &project, &work],
        )
        .await?
        .get(0);
    let recorded = latest(tx, tenant, project, work).await?;
    let current = governed
        .work
        .binding(&governed.policy, &contract_hash)
        .map_err(|_| PgError::SourceDivergence)?;
    let status = admission_status(
        &governed.policy,
        &governed.work,
        &contract_hash,
        completed,
        recorded.as_ref(),
    )
    .map_err(|_| PgError::SourceDivergence)?;
    Ok(Some((status, current, governed)))
}

/// Governed work needs a current approved decision before it is claimed, dispatched or executed.
/// Observing a policy reports and holds nothing; recovery paths never call this.
pub(crate) async fn require_current(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    snapshot: &str,
    work: &str,
) -> PgResult<()> {
    let Some((status, _, _)) = status(tx, tenant, project, snapshot, work).await? else {
        return Ok(());
    };
    if !status.enforced {
        return Ok(());
    }
    match status.diagnostics.first() {
        Some(first) => Err(PgError::AdmissionRequired {
            code: first.code,
            detail: first.detail.clone(),
        }),
        None => Ok(()),
    }
}

/// What a decider reads before deciding: the state of the work, why, and the fingerprint of the
/// scope that a decision will be checked against.
pub(crate) async fn read(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    snapshot: &str,
    work: &str,
) -> PgResult<Value> {
    let Some((status, current, governed)) = status(tx, tenant, project, snapshot, work).await?
    else {
        return Ok(
            serde_json::json!({"governed": false, "basis": "the snapshot declares no governance"}),
        );
    };
    let fingerprint = binding_fingerprint(&current).map_err(|_| PgError::SourceDivergence)?;
    Ok(serde_json::json!({
        "governed": status.governed, "enforced": status.enforced, "state": status.state, "mode": status.mode,
        "admitted": status.facts.admitted, "independent_review_required": status.independent_review_required,
        "diagnostics": status.diagnostics, "facts": status.facts, "basis": status.basis,
        "binding": current, "binding_fingerprint": fingerprint,
        "governance_hash": governed.governance_hash,
        "definition_summary": governed.work.summary,
    }))
}
