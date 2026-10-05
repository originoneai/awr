//! Explicit lower-trust settlement of the exact admitted workspace resources.
//! Caller assertions never attest to an OS sandbox or external repository effects.
use super::*;
use awr_team::{ExecutionSettlementMode, ExecutionSettlementPolicy};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Declaration {
    pub workspace_id: String,
    pub input_digest: String,
    pub environment_digest: String,
    pub claim_id: String,
    pub expected_fence: String,
    pub expected_lease_version: String,
    pub executor_stopped: Option<bool>,
    pub no_external_effects: Option<bool>,
}

impl Declaration {
    pub(super) fn validate(&self) -> PgResult<()> {
        ExecutionSettlementPolicy {
            mode: ExecutionSettlementMode::IndependentWorkspaceV1,
            workspace_id: self.workspace_id.clone(),
        }
        .validate()
        .map_err(|_| invalid())?;
        if !digest(&self.input_digest)
            || !digest(&self.environment_digest)
            || !identity(&self.claim_id)
            || version(&self.expected_fence)? == 0
            || version(&self.expected_lease_version)? == 0
        {
            return Err(invalid());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ResourceProof {
    reservation_id: String,
    kind: String,
    key: String,
    workspace_id: String,
    fence: String,
    admission_lease_version: String,
}

pub(super) struct Settled {
    pub policy: ExecutionSettlementPolicy,
    pub declaration: Declaration,
    pub resources: Vec<ResourceProof>,
}

pub(super) fn normalized_scope(paths: &[String]) -> PgResult<Vec<String>> {
    if paths.is_empty() || paths.len() > 128 || paths.iter().any(|p| !canonical_path(p)) {
        return Err(PgError::ScopeExceeded);
    }
    let normalized = paths
        .iter()
        .map(|p| crate::graph::normalize_resource_key("dir", p))
        .collect::<PgResult<Vec<_>>>()?;
    if normalized.iter().collect::<BTreeSet<_>>().len() != normalized.len() {
        return Err(PgError::ScopeExceeded);
    }
    Ok(normalized)
}

pub(super) fn policy(run: &Row) -> PgResult<Option<ExecutionSettlementPolicy>> {
    run.get::<_, Option<Value>>("settlement_policy_json")
        .map(|value| {
            let policy: ExecutionSettlementPolicy =
                serde_json::from_value(value).map_err(|_| PgError::SourceDivergence)?;
            policy.validate().map_err(|_| PgError::SourceDivergence)?;
            Ok(policy)
        })
        .transpose()
}

async fn exact_resources(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    run: &Row,
    policy: &ExecutionSettlementPolicy,
    active: bool,
) -> PgResult<Option<Vec<ResourceProof>>> {
    let declared: Vec<String> = serde_json::from_value(run.get("declared_scope_json"))
        .map_err(|_| PgError::SourceDivergence)?;
    let paths = normalized_scope(&declared)?;
    let execution: String = run.get("id");
    let work: String = run.get("work_id");
    let fence: i64 = run.get("fence");
    let generation: i64 = run
        .get::<_, Option<i64>>("admission_lease_version")
        .ok_or(PgError::SourceDivergence)?;
    let rows = tx
        .query(
            "SELECT id,execution_id,resource_kind,canonical_key,worktree_id,lease_generation,fence,state
             FROM awr_team.resource_reservations
             WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3
               AND (($5 AND state IN ('reserved','unknown')) OR (NOT $5 AND execution_id=$4))
             ORDER BY id",
            &[&tenant, &project, &work, &execution, &active],
        )
        .await?;
    if rows.len() != paths.len() {
        return Ok(None);
    }
    let mut keys = BTreeSet::new();
    let mut resources = Vec::new();
    for row in rows {
        let key: String = row.get(3);
        if row.get::<_, Option<String>>(1).as_deref() != Some(&execution)
            || row.get::<_, String>(2) != "dir"
            || !paths.contains(&key)
            || !keys.insert(key.clone())
            || row.get::<_, String>(4) != policy.workspace_id
            || row.get::<_, i64>(5) != generation
            || row.get::<_, i64>(6) != fence
            || row.get::<_, String>(7) != if active { "reserved" } else { "released" }
        {
            return Ok(None);
        }
        resources.push(ResourceProof {
            reservation_id: row.get(0),
            kind: "dir".into(),
            key,
            workspace_id: policy.workspace_id.clone(),
            fence: fence.to_string(),
            admission_lease_version: generation.to_string(),
        });
    }
    Ok(Some(resources))
}

pub(super) async fn evaluate(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    ownership: i64,
    run: &Row,
    contract: &awr_team::WorkContract,
    declaration: Option<&Declaration>,
    outcome: &str,
    scope_violation: bool,
) -> PgResult<Option<Settled>> {
    let Some(declaration) = declaration else {
        return Ok(None);
    };
    let stored = policy(run)?.ok_or(PgError::PreconditionsChanged)?;
    // Invalid identity declarations fail before any caller receipt is written.
    if run.get::<_, Option<String>>("admission_mode").as_deref() != Some("caller_managed")
        || declaration.workspace_id != stored.workspace_id
        || run.get::<_, Option<String>>("input_digest").as_deref()
            != Some(&declaration.input_digest)
        || run.get::<_, Option<String>>("claim_id").as_deref() != Some(&declaration.claim_id)
        || run.get::<_, i64>("fence") != version(&declaration.expected_fence)?
        || run.get::<_, i64>("claim_lease_version") != version(&declaration.expected_lease_version)?
    {
        return Err(PgError::PreconditionsChanged);
    }
    let generation = run
        .get::<_, Option<i64>>("admission_lease_version")
        .ok_or(PgError::PreconditionsChanged)?;
    if generation > version(&declaration.expected_lease_version)? {
        return Err(PgError::PreconditionsChanged);
    }
    if declaration.executor_stopped != Some(true)
        || declaration.no_external_effects != Some(true)
        || outcome == "unknown"
        || scope_violation
        || run.get::<_, String>("state") != "running"
        || run.get::<_, bool>("recovery_blocked")
        || !run.get::<_, bool>("lease_live")
        || run.get::<_, String>("contract_hash") != command.expected_contract_hash
        || contract.execution_settlement.as_ref() != Some(&stored)
    {
        return Ok(None);
    }
    match claims::require_live(
        tx,
        tenant,
        project,
        auth,
        command,
        ownership,
        run.get::<_, Option<String>>("session_id")
            .as_deref()
            .ok_or(PgError::Forbidden)?,
        &declaration.claim_id,
        &declaration.expected_fence,
        &declaration.expected_lease_version,
    )
    .await
    {
        Ok(_) => {}
        Err(PgError::LeaseExpired) => return Ok(None),
        Err(error) => return Err(error),
    }
    let protected: bool = tx.query_one(
        "SELECT
          NOT EXISTS(SELECT 1 FROM awr_team.work_contracts c JOIN awr_team.work_scopes s
            ON s.tenant_id=c.tenant_id AND s.project_id=c.project_id AND s.id=c.scope_id
            WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.snapshot_id=$3 AND c.scope_id='main'
              AND c.work_id=$4 AND c.definition_state='enabled' AND s.status='active')
          OR EXISTS(SELECT 1 FROM awr_team.executions WHERE tenant_id=$1 AND project_id=$2 AND work_id=$4
            AND id<>$5 AND state NOT IN ('succeeded','failed','cancelled'))
          OR EXISTS(SELECT 1 FROM awr_team.outbox WHERE tenant_id=$1 AND project_id=$2 AND aggregate_id=$5)
          OR EXISTS(SELECT 1 FROM awr_team.execution_receipts WHERE tenant_id=$1 AND project_id=$2 AND execution_id=$5)
          OR EXISTS(SELECT 1 FROM awr_team.planning_change_action_blocks WHERE tenant_id=$1 AND project_id=$2 AND work_id=$4 AND active=true)
          OR EXISTS(SELECT 1 FROM awr_team.dependency_bindings WHERE tenant_id=$1 AND project_id=$2 AND downstream_work_id=$4 AND valid=false)",
        &[&tenant, &project, &auth.snapshot, &command.work_id, &run.get::<_, String>("id")],
    ).await?.get(0);
    if protected {
        return Ok(None);
    }
    let Some(resources) = exact_resources(tx, tenant, project, run, &stored, true).await? else {
        return Ok(None);
    };
    Ok(Some(Settled {
        policy: stored,
        declaration: declaration.clone(),
        resources,
    }))
}

pub(super) async fn release(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    run: &Row,
    settled: &Settled,
) -> PgResult<()> {
    let ids: Vec<_> = settled
        .resources
        .iter()
        .map(|r| r.reservation_id.clone())
        .collect();
    let affected = tx
        .execute(
            "UPDATE awr_team.resource_reservations SET state='released'
         WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND execution_id=$4
           AND id=ANY($5) AND state='reserved'",
            &[
                &tenant,
                &project,
                &run.get::<_, String>("work_id"),
                &run.get::<_, String>("id"),
                &ids,
            ],
        )
        .await?;
    if affected != ids.len() as u64 {
        return Err(PgError::PreconditionsChanged);
    }
    Ok(())
}

impl Settled {
    pub(super) fn bind_payload(&self, payload: &mut Value, run: &Row, session_version: &str) {
        let fields = json!({
            "execution_id": run.get::<_, String>("id"), "work_id": run.get::<_, String>("work_id"),
            "claim_id": self.declaration.claim_id, "fence": self.declaration.expected_fence,
            "input_digest": self.declaration.input_digest, "environment_digest": self.declaration.environment_digest,
            "session_version": session_version, "admission_mode": "caller_managed",
            "admission_lease_version": run.get::<_, Option<i64>>("admission_lease_version").map(|v| v.to_string()),
            "settlement_policy": self.policy, "workspace_settlement": self.declaration,
            "terminal_reported": true, "effects_settled": true, "artifact_verified": false,
            "settlement_scope": "admitted_workspace_paths", "resource_proof": self.resources,
        });
        payload
            .as_object_mut()
            .expect("caller payload")
            .extend(fields.as_object().unwrap().clone());
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceReceipt {
    outcome: String,
    output_digest: Option<String>,
    observed_paths: Vec<String>,
    note: String,
    client_id: String,
    session_id: String,
    session_version: String,
    workstream_id: String,
    ownership_version: String,
    execution_version: String,
    coordinator_epoch: String,
    contract_hash: String,
    scope_violation: bool,
    execution_id: String,
    work_id: String,
    claim_id: String,
    fence: String,
    input_digest: String,
    environment_digest: String,
    admission_mode: String,
    admission_lease_version: String,
    settlement_policy: ExecutionSettlementPolicy,
    workspace_settlement: Declaration,
    terminal_reported: bool,
    effects_settled: bool,
    artifact_verified: bool,
    settlement_scope: String,
    resource_proof: Vec<ResourceProof>,
}

/// The caller supplies a digest only after re-reading and verifying artifact bytes.
pub(crate) async fn verify_receipt(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    run: &Row,
    payload: &Value,
    artifact_digest: Option<&str>,
) -> PgResult<Value> {
    let receipt: WorkspaceReceipt =
        serde_json::from_value(payload.clone()).map_err(|_| PgError::EvidenceInvalid)?;
    receipt
        .workspace_settlement
        .validate()
        .map_err(|_| PgError::EvidenceInvalid)?;
    receipt
        .settlement_policy
        .validate()
        .map_err(|_| PgError::EvidenceInvalid)?;
    let stored = policy(run)
        .map_err(|_| PgError::EvidenceInvalid)?
        .ok_or(PgError::EvidenceInvalid)?;
    let declaration = &receipt.workspace_settlement;
    let terminal_version = version(&receipt.execution_version)
        .map_err(|_| PgError::EvidenceInvalid)?
        .checked_add(1);
    if receipt.outcome != "succeeded"
        || !receipt.terminal_reported
        || !receipt.effects_settled
        || receipt.artifact_verified
        || !run.get::<_, bool>("terminal_reported")
        || !run.get::<_, bool>("workspace_effects_settled")
        || receipt.scope_violation
        || receipt.settlement_scope != "admitted_workspace_paths"
        || receipt.admission_mode != "caller_managed"
        || run.get::<_, Option<String>>("admission_mode").as_deref() != Some("caller_managed")
        || receipt.execution_id != run.get::<_, String>("id")
        || receipt.work_id != command.work_id
        || receipt.client_id
            != run
                .get::<_, Option<String>>("executor_client_id")
                .unwrap_or_default()
        || receipt.session_id
            != run
                .get::<_, Option<String>>("session_id")
                .unwrap_or_default()
        || version(&receipt.session_version).map_err(|_| PgError::EvidenceInvalid)? == 0
        || receipt.workstream_id != command.workstream_id.to_string()
        || receipt.ownership_version != command.expected_ownership_version
        || terminal_version != Some(run.get::<_, i64>("execution_version"))
        || receipt.coordinator_epoch != auth.epoch
        || run.get::<_, Option<String>>("coordinator_epoch").as_deref()
            != Some(&receipt.coordinator_epoch)
        || receipt.contract_hash != command.expected_contract_hash
        || receipt.contract_hash != run.get::<_, String>("contract_hash")
        || receipt.claim_id != run.get::<_, Option<String>>("claim_id").unwrap_or_default()
        || receipt.claim_id != declaration.claim_id
        || receipt.fence != declaration.expected_fence
        || version(&receipt.fence).map_err(|_| PgError::EvidenceInvalid)?
            != run.get::<_, i64>("fence")
        || Some(version(&receipt.admission_lease_version).map_err(|_| PgError::EvidenceInvalid)?)
            != run.get::<_, Option<i64>>("admission_lease_version")
        || version(&declaration.expected_lease_version).map_err(|_| PgError::EvidenceInvalid)?
            < version(&receipt.admission_lease_version).map_err(|_| PgError::EvidenceInvalid)?
        || receipt.settlement_policy != stored
        || declaration.workspace_id != stored.workspace_id
        || declaration.executor_stopped != Some(true)
        || declaration.no_external_effects != Some(true)
        || receipt.input_digest != declaration.input_digest
        || run.get::<_, Option<String>>("input_digest").as_deref() != Some(&receipt.input_digest)
        || receipt.environment_digest != declaration.environment_digest
        || run
            .get::<_, Option<String>>("environment_digest")
            .as_deref()
            != Some(&receipt.environment_digest)
        || receipt.output_digest.as_deref() != artifact_digest
        || artifact_digest.is_none()
        || run.get::<_, Option<String>>("result_digest").as_deref() != artifact_digest
        || run.get::<_, Option<Value>>("observed_paths_json") != Some(json!(receipt.observed_paths))
        || receipt.note.trim().is_empty()
        || receipt.note.len() > 4096
        || receipt.note.contains('\0')
    {
        return Err(PgError::EvidenceInvalid);
    }
    let current: Value = tx
        .query_one(
            "SELECT contract_json FROM awr_team.work_contracts WHERE tenant_id=$1 AND project_id=$2
         AND snapshot_id=$3 AND scope_id='main' AND work_id=$4",
            &[&tenant, &project, &auth.snapshot, &command.work_id],
        )
        .await?
        .get(0);
    let contract: awr_team::WorkContract =
        serde_json::from_value(current).map_err(|_| PgError::EvidenceInvalid)?;
    let declared: Vec<String> = serde_json::from_value(run.get("declared_scope_json"))
        .map_err(|_| PgError::EvidenceInvalid)?;
    if contract.execution_settlement.as_ref() != Some(&stored)
        || require_paths(&contract, &declared).is_err()
        || receipt.observed_paths.len() > 128
        || receipt.observed_paths.iter().any(|path| {
            !canonical_path(path)
                || !declared.iter().any(|scope| {
                    canonical_path(scope) && crate::graph::path_within_scope(scope, path)
                })
        })
    {
        return Err(PgError::EvidenceInvalid);
    }
    let resources = exact_resources(tx, tenant, project, run, &stored, false)
        .await
        .map_err(|_| PgError::EvidenceInvalid)?
        .ok_or(PgError::EvidenceInvalid)?;
    if resources != receipt.resource_proof {
        return Err(PgError::EvidenceInvalid);
    }
    Ok(
        json!({"execution_basis": "caller_asserted_workspace_settled", "settlement_mode": "independent_workspace_v1",
        "workspace_id": stored.workspace_id, "terminal_reported": true, "artifact_verified": true,
        "effects_settled": true, "settlement_scope": "admitted_workspace_paths"}),
    )
}
