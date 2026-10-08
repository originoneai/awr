//! Team PG persistence for confirmed long-term handoffs (WS-017).
use crate::error::{PgError, PgResult};
use crate::tx::{bind_workstream_scope, new_id};
use awr_core::{
    AcceptHandoffRequest, CancelHandoffRequest, ExecutionInstance, HandoffDuty, HandoffKind,
    HandoffReceipt, HandoffStatus, InspectHandoffRequest, PersonId, ProposeHandoffRequest,
    RejectHandoffRequest, TeamHandoff, TimeoutHandoffRequest, apply_handoff_accept,
    apply_handoff_cancel, apply_handoff_inspect, apply_handoff_propose, apply_handoff_reject,
    apply_handoff_timeout,
};
use serde_json::Value;
use tokio_postgres::Transaction;

fn map_core(err: awr_core::Error) -> PgError {
    match err {
        awr_core::Error::RevisionConflict { .. } => PgError::PreconditionsChanged,
        awr_core::Error::ClaimConflict(_) => PgError::ClaimHeld,
        awr_core::Error::RuleViolation(_) => PgError::Forbidden,
        awr_core::Error::NotFound(_) => PgError::Protocol(format!("not found: {err}")),
        awr_core::Error::InvalidInput(m) => PgError::Protocol(m),
        other => PgError::Protocol(other.to_string()),
    }
}

fn status_str(s: HandoffStatus) -> &'static str {
    match s {
        HandoffStatus::Proposed => "proposed",
        HandoffStatus::Inspected => "inspected",
        HandoffStatus::Accepted => "accepted",
        HandoffStatus::Rejected => "rejected",
        HandoffStatus::Cancelled => "cancelled",
        HandoffStatus::TimedOut => "timed_out",
    }
}

fn kind_str(k: HandoffKind) -> &'static str {
    match k {
        HandoffKind::Execution => "execution",
        HandoffKind::Responsibility => "responsibility",
    }
}

pub struct HandoffStore {
    pool: crate::PgPool,
}

impl HandoffStore {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            pool: crate::PgPool::new(url),
        }
    }

    pub fn from_config(config: tokio_postgres::Config) -> Self {
        Self {
            pool: crate::PgPool::from_config(config),
        }
    }

    async fn connect(&self) -> PgResult<crate::PgClient> {
        self.pool.get().await
    }

    pub async fn get(
        &self,
        tenant: &str,
        project: &str,
        handoff_id: &str,
    ) -> PgResult<Option<TeamHandoff>> {
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_workstream_scope(&tx, tenant, project).await?;
        let row = tx
            .query_opt(
                "SELECT body_json FROM awr_team.team_handoffs
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant, &project, &handoff_id],
            )
            .await?;
        let out = match row {
            None => None,
            Some(r) => {
                let v: Value = r.get(0);
                Some(serde_json::from_value(v).map_err(|e| PgError::Protocol(e.to_string()))?)
            }
        };
        tx.commit().await?;
        Ok(out)
    }

    pub async fn duty(
        &self,
        tenant: &str,
        project: &str,
        handoff_id: &str,
        now_ms: i64,
    ) -> PgResult<HandoffDuty> {
        let h = self
            .get(tenant, project, handoff_id)
            .await?
            .ok_or_else(|| PgError::Protocol("handoff not found".into()))?;
        h.duty_at(now_ms).map_err(map_core)
    }

    pub async fn propose(
        &self,
        tenant: &str,
        project: &str,
        work_id: &str,
        from_person: &PersonId,
        req: &ProposeHandoffRequest,
    ) -> PgResult<(TeamHandoff, HandoffReceipt)> {
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_workstream_scope(&tx, tenant, project).await?;
        // Project barrier before any handoff or task row, as on the command path
        // (see lock_order.rs); the task row is locked later in the transfer commit.
        crate::agent_authorization::lock_project(&tx, tenant, project).await?;
        if let Some(receipt) =
            load_receipt(&tx, tenant, project, &req.request_key, "propose").await?
        {
            let h = load_tx(&tx, tenant, project, &receipt.handoff_id)
                .await?
                .ok_or_else(|| PgError::Protocol("handoff missing for receipt".into()))?;
            tx.commit().await?;
            return Ok((h, receipt));
        }
        ensure_person(&tx, tenant, project, from_person.as_str()).await?;
        ensure_person(&tx, tenant, project, req.to_person_id.as_str()).await?;
        let handoff =
            apply_handoff_propose(project, work_id, from_person, req).map_err(map_core)?;
        persist(&tx, tenant, project, &handoff).await?;
        let receipt = record(&tx, tenant, project, &handoff, &req.request_key, "propose").await?;
        tx.commit().await?;
        Ok((handoff, receipt))
    }

    pub async fn inspect(
        &self,
        tenant: &str,
        project: &str,
        req: &InspectHandoffRequest,
    ) -> PgResult<(TeamHandoff, HandoffReceipt)> {
        self.mutate(
            tenant,
            project,
            &req.request_key,
            "inspect",
            &req.handoff_id,
            |before| apply_handoff_inspect(before, req).map_err(map_core),
        )
        .await
    }

    pub async fn accept(
        &self,
        tenant: &str,
        project: &str,
        req: &AcceptHandoffRequest,
    ) -> PgResult<(TeamHandoff, HandoffReceipt)> {
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_workstream_scope(&tx, tenant, project).await?;
        // Project barrier before any handoff or task row, as on the command path
        // (see lock_order.rs); the task row is locked later in the transfer commit.
        crate::agent_authorization::lock_project(&tx, tenant, project).await?;
        if let Some(receipt) =
            load_receipt(&tx, tenant, project, &req.request_key, "accept").await?
        {
            if receipt.handoff_id != req.handoff_id {
                return Err(PgError::IdempotencyConflict);
            }
            let h = load_tx(&tx, tenant, project, &receipt.handoff_id)
                .await?
                .ok_or_else(|| PgError::Protocol("handoff missing for receipt".into()))?;
            let again = apply_handoff_accept(&h, req).map_err(map_core)?;
            if again != h {
                return Err(PgError::IdempotencyConflict);
            }
            tx.commit().await?;
            return Ok((h, receipt));
        }
        let before = load_for_update(&tx, tenant, project, &req.handoff_id)
            .await?
            .ok_or_else(|| PgError::Protocol("handoff not found".into()))?;
        let mut live_req = req.clone();
        live_req.unknown_executions_open =
            unknown_open(&tx, tenant, project, &before.work_item_id).await?;
        if req.expected_current_fence.is_some() {
            live_req.live_fence = live_fence(&tx, tenant, project, &before.work_item_id).await?;
        }
        let was_open = before.status.is_open();
        let next = apply_handoff_accept(&before, &live_req).map_err(map_core)?;
        if was_open && next.status == HandoffStatus::Accepted {
            commit_accepted_transfer(&tx, tenant, project, &before, &next).await?;
        }
        persist(&tx, tenant, project, &next).await?;
        let receipt = record(&tx, tenant, project, &next, &req.request_key, "accept").await?;
        tx.commit().await?;
        Ok((next, receipt))
    }

    pub async fn reject(
        &self,
        tenant: &str,
        project: &str,
        req: &RejectHandoffRequest,
    ) -> PgResult<(TeamHandoff, HandoffReceipt)> {
        self.mutate(
            tenant,
            project,
            &req.request_key,
            "reject",
            &req.handoff_id,
            |before| apply_handoff_reject(before, req).map_err(map_core),
        )
        .await
    }

    pub async fn cancel(
        &self,
        tenant: &str,
        project: &str,
        req: &CancelHandoffRequest,
    ) -> PgResult<(TeamHandoff, HandoffReceipt)> {
        self.mutate(
            tenant,
            project,
            &req.request_key,
            "cancel",
            &req.handoff_id,
            |before| apply_handoff_cancel(before, req).map_err(map_core),
        )
        .await
    }

    pub async fn timeout(
        &self,
        tenant: &str,
        project: &str,
        req: &TimeoutHandoffRequest,
    ) -> PgResult<(TeamHandoff, HandoffReceipt)> {
        self.mutate(
            tenant,
            project,
            &req.request_key,
            "timeout",
            &req.handoff_id,
            |before| apply_handoff_timeout(before, req).map_err(map_core),
        )
        .await
    }

    async fn mutate<F>(
        &self,
        tenant: &str,
        project: &str,
        request_key: &str,
        op: &str,
        handoff_id: &str,
        transition: F,
    ) -> PgResult<(TeamHandoff, HandoffReceipt)>
    where
        F: FnOnce(&TeamHandoff) -> PgResult<TeamHandoff>,
    {
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_workstream_scope(&tx, tenant, project).await?;
        // Project barrier before any handoff or task row, as on the command path
        // (see lock_order.rs); the task row is locked later in the transfer commit.
        crate::agent_authorization::lock_project(&tx, tenant, project).await?;
        if let Some(receipt) = load_receipt(&tx, tenant, project, request_key, op).await? {
            if receipt.handoff_id != handoff_id {
                return Err(PgError::IdempotencyConflict);
            }
            let h = load_tx(&tx, tenant, project, &receipt.handoff_id)
                .await?
                .ok_or_else(|| PgError::Protocol("handoff missing for receipt".into()))?;
            tx.commit().await?;
            return Ok((h, receipt));
        }
        let before = load_for_update(&tx, tenant, project, handoff_id)
            .await?
            .ok_or_else(|| PgError::Protocol("handoff not found".into()))?;
        let next = transition(&before)?;
        persist(&tx, tenant, project, &next).await?;
        let receipt = record(&tx, tenant, project, &next, request_key, op).await?;
        tx.commit().await?;
        Ok((next, receipt))
    }
}

/// Apply authoritative responsibility / execution effects after a newly accepted handoff.
/// Must run in the same transaction as the handoff row persist.
pub(crate) async fn commit_accepted_transfer(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    before: &TeamHandoff,
    after: &TeamHandoff,
) -> PgResult<()> {
    if after.status != HandoffStatus::Accepted || !before.status.is_open() {
        return Ok(());
    }
    let work_id = after.work_item_id.as_str();
    let successor = after
        .accepted_successor
        .as_ref()
        .ok_or_else(|| PgError::Protocol("accepted handoff missing successor execution".into()))?;
    ensure_person(tx, tenant, project, after.to_person_id.as_str()).await?;
    ensure_person(tx, tenant, project, after.from_person_id.as_str()).await?;
    ensure_person(tx, tenant, project, successor.person_id().as_str()).await?;

    let row = tx
        .query_opt(
            "SELECT owner_person_id, executor_kind, executor_person_id, version
             FROM awr_team.task_responsibilities
             WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 FOR UPDATE",
            &[&tenant, &project, &work_id],
        )
        .await?;

    match after.kind {
        HandoffKind::Responsibility => {
            let owner = row.as_ref().and_then(|r| r.get::<_, Option<String>>(0));
            if let Some(owner) = owner.as_deref() {
                if owner != after.from_person_id.as_str() {
                    return Err(PgError::Forbidden);
                }
            } else if row.is_some() {
                return Err(PgError::Forbidden);
            }
            let version: i64 = row.map(|r| r.get::<_, i64>(3)).unwrap_or(0);
            upsert_responsibility_owner(
                tx,
                tenant,
                project,
                work_id,
                after.to_person_id.as_str(),
                version + 1,
            )
            .await?;
        }
        HandoffKind::Execution => {
            if let Some(r) = &row {
                let exec_person: Option<String> = r.get(2);
                if let Some(current) = exec_person.as_deref() {
                    if current != after.from_person_id.as_str()
                        && current != after.package.current_person_id.as_str()
                    {
                        return Err(PgError::Forbidden);
                    }
                }
            }
            let version: i64 = row.as_ref().map(|r| r.get::<_, i64>(3)).unwrap_or(0);
            upsert_responsibility_executor(
                tx,
                tenant,
                project,
                work_id,
                after.from_person_id.as_str(),
                successor,
                version + 1,
            )
            .await?;
            tx.execute(
                "UPDATE awr_team.claims SET state='handed_off'
                 WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3
                   AND state='active'",
                &[&tenant, &project, &work_id],
            )
            .await?;
            bump_fence(tx, tenant, project, work_id).await?;
        }
    }
    Ok(())
}

async fn upsert_responsibility_owner(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work_id: &str,
    owner: &str,
    version: i64,
) -> PgResult<()> {
    tx.execute(
        "INSERT INTO awr_team.task_responsibilities(
            tenant_id,project_id,work_id,owner_person_id,version,personal_mode_default)
         VALUES($1,$2,$3,$4,$5,false)
         ON CONFLICT(tenant_id,project_id,work_id) DO UPDATE SET
            owner_person_id=EXCLUDED.owner_person_id,
            pending_kind=NULL,
            pending_person_id=NULL,
            pending_legacy_ref=NULL,
            pending_transfer_request_key=NULL,
            pending_detail=NULL,
            version=EXCLUDED.version,
            updated_at=clock_timestamp()",
        &[&tenant, &project, &work_id, &owner, &version],
    )
    .await?;
    Ok(())
}

async fn upsert_responsibility_executor(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work_id: &str,
    owner_fallback: &str,
    successor: &ExecutionInstance,
    version: i64,
) -> PgResult<()> {
    let (kind, person, agent, binding): (&str, &str, Option<&str>, Option<&str>) = match successor {
        ExecutionInstance::Person { person_id } => ("person", person_id.as_str(), None, None),
        ExecutionInstance::AgentRun {
            person_id,
            agent_id,
            binding_id,
        } => (
            "agent_run",
            person_id.as_str(),
            Some(agent_id.as_str()),
            Some(binding_id.as_str()),
        ),
    };
    tx.execute(
        "INSERT INTO awr_team.task_responsibilities(
            tenant_id,project_id,work_id,owner_person_id,executor_kind,executor_person_id,
            executor_agent_id,executor_binding_id,version,personal_mode_default)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,false)
         ON CONFLICT(tenant_id,project_id,work_id) DO UPDATE SET
            executor_kind=EXCLUDED.executor_kind,
            executor_person_id=EXCLUDED.executor_person_id,
            executor_agent_id=EXCLUDED.executor_agent_id,
            executor_binding_id=EXCLUDED.executor_binding_id,
            version=EXCLUDED.version,
            updated_at=clock_timestamp()",
        &[
            &tenant,
            &project,
            &work_id,
            &owner_fallback,
            &kind,
            &person,
            &agent,
            &binding,
            &version,
        ],
    )
    .await?;
    Ok(())
}

async fn ensure_person(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    person_id: &str,
) -> PgResult<()> {
    tx.execute(
        "INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
         VALUES($1,$2,$3,$3,'active')
         ON CONFLICT(tenant_id,project_id,id) DO NOTHING",
        &[&tenant, &project, &person_id],
    )
    .await?;
    Ok(())
}

async fn unknown_open(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work_id: &str,
) -> PgResult<bool> {
    // Table may exist from schema >=4; COUNT on empty is fine.
    let n: i64 = tx
        .query_one(
            "SELECT COUNT(*)::bigint FROM awr_team.executions
             WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='unknown'",
            &[&tenant, &project, &work_id],
        )
        .await?
        .get(0);
    Ok(n > 0)
}

async fn live_fence(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work_id: &str,
) -> PgResult<Option<i64>> {
    Ok(tx
        .query_opt(
            "SELECT last_fence FROM awr_team.work_runtime
             WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3",
            &[&tenant, &project, &work_id],
        )
        .await?
        .map(|r| r.get(0)))
}

async fn bump_fence(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work_id: &str,
) -> PgResult<()> {
    let _ = tx
        .execute(
            "UPDATE awr_team.work_runtime SET last_fence = last_fence + 1
             WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3",
            &[&tenant, &project, &work_id],
        )
        .await?;
    Ok(())
}

async fn load_tx(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    id: &str,
) -> PgResult<Option<TeamHandoff>> {
    let row = tx
        .query_opt(
            "SELECT body_json FROM awr_team.team_handoffs
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &id],
        )
        .await?;
    decode(row)
}

async fn load_for_update(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    id: &str,
) -> PgResult<Option<TeamHandoff>> {
    let row = tx
        .query_opt(
            "SELECT body_json FROM awr_team.team_handoffs
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3 FOR UPDATE",
            &[&tenant, &project, &id],
        )
        .await?;
    decode(row)
}

fn decode(row: Option<tokio_postgres::Row>) -> PgResult<Option<TeamHandoff>> {
    match row {
        None => Ok(None),
        Some(r) => {
            let v: Value = r.get(0);
            Ok(Some(
                serde_json::from_value(v).map_err(|e| PgError::Protocol(e.to_string()))?,
            ))
        }
    }
}

async fn persist(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    h: &TeamHandoff,
) -> PgResult<()> {
    let body = serde_json::to_value(h).map_err(|e| PgError::Protocol(e.to_string()))?;
    let package = serde_json::to_value(&h.package).map_err(|e| PgError::Protocol(e.to_string()))?;
    let proposed = h
        .proposed_successor
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|e| PgError::Protocol(e.to_string()))?;
    let accepted = h
        .accepted_successor
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|e| PgError::Protocol(e.to_string()))?;
    tx.execute(
        "INSERT INTO awr_team.team_handoffs(
            tenant_id,project_id,id,work_id,kind,status,version,from_person_id,to_person_id,
            package_json,proposed_successor_json,accepted_successor_json,proposer_execution_id,
            proposer_fence,expires_at_ms,created_at_ms,updated_at_ms,inspected_at_ms,
            terminal_at_ms,terminal_reason,accept_request_key,body_json)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22)
         ON CONFLICT(tenant_id,project_id,id) DO UPDATE SET
            status=EXCLUDED.status,
            version=EXCLUDED.version,
            package_json=EXCLUDED.package_json,
            proposed_successor_json=EXCLUDED.proposed_successor_json,
            accepted_successor_json=EXCLUDED.accepted_successor_json,
            updated_at_ms=EXCLUDED.updated_at_ms,
            inspected_at_ms=EXCLUDED.inspected_at_ms,
            terminal_at_ms=EXCLUDED.terminal_at_ms,
            terminal_reason=EXCLUDED.terminal_reason,
            accept_request_key=EXCLUDED.accept_request_key,
            body_json=EXCLUDED.body_json",
        &[
            &tenant,
            &project,
            &h.id,
            &h.work_item_id,
            &kind_str(h.kind),
            &status_str(h.status),
            &(h.version as i64),
            &h.from_person_id.as_str(),
            &h.to_person_id.as_str(),
            &package,
            &proposed,
            &accepted,
            &h.proposer_execution_id,
            &h.proposer_fence,
            &h.expires_at_ms,
            &h.created_at_ms,
            &h.updated_at_ms,
            &h.inspected_at_ms,
            &h.terminal_at_ms,
            &h.terminal_reason,
            &h.accept_request_key,
            &body,
        ],
    )
    .await
    .map_err(|e| {
        if e.code()
            .map(|c| *c == tokio_postgres::error::SqlState::UNIQUE_VIOLATION)
            .unwrap_or(false)
        {
            PgError::PreconditionsChanged
        } else {
            PgError::Db(e)
        }
    })?;
    Ok(())
}

async fn load_receipt(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    request_key: &str,
    op: &str,
) -> PgResult<Option<HandoffReceipt>> {
    let row = tx
        .query_opt(
            "SELECT handoff_id, event_id, op FROM awr_team.team_handoff_receipts
             WHERE tenant_id=$1 AND project_id=$2 AND request_key=$3",
            &[&tenant, &project, &request_key],
        )
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let handoff_id: String = row.get(0);
    let event_id: String = row.get(1);
    let stored_op: String = row.get(2);
    if stored_op != op {
        return Err(PgError::IdempotencyConflict);
    }
    let h = load_tx(tx, tenant, project, &handoff_id)
        .await?
        .ok_or_else(|| PgError::Protocol("handoff missing for receipt".into()))?;
    let duty = h.duty_at(h.updated_at_ms).map_err(map_core)?;
    Ok(Some(HandoffReceipt {
        request_key: request_key.into(),
        handoff_id,
        op: op.into(),
        event_id,
        replayed: true,
        status: h.status,
        version: h.version,
        duty,
    }))
}

async fn record(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    h: &TeamHandoff,
    request_key: &str,
    op: &str,
) -> PgResult<HandoffReceipt> {
    let event_id = new_id();
    tx.execute(
        "INSERT INTO awr_team.team_handoff_receipts(
            tenant_id,project_id,request_key,handoff_id,op,event_id,replayed,created_at_ms)
         VALUES($1,$2,$3,$4,$5,$6,FALSE,$7)",
        &[
            &tenant,
            &project,
            &request_key,
            &h.id,
            &op,
            &event_id,
            &h.updated_at_ms,
        ],
    )
    .await?;
    let duty = h.duty_at(h.updated_at_ms).map_err(map_core)?;
    Ok(HandoffReceipt {
        request_key: request_key.into(),
        handoff_id: h.id.clone(),
        op: op.into(),
        event_id,
        replayed: false,
        status: h.status,
        version: h.version,
        duty,
    })
}
