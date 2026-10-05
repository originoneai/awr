//! Team PG persistence for person responsibility and execution-instance assignment.
//! Extends coordination without duplicating claim leases: execution claim never
//! steals sole ownership. Person↔agent bindings are explicit; actor.kind is ignored.
use crate::error::{PgError, PgResult};
use crate::tx::{bind_workstream_scope, new_id};
use awr_core::{
    AcceptResponsibilityRequest, AssignResponsibilityRequest, BindingStatus, ClaimAvailableRequest,
    ClaimExecutionRequest, ExecutionInstance, PersonAgentBinding, PersonId,
    ResponsibilityEventType, ResponsibilityPending, ResponsibilityPendingKind,
    ResponsibilityReceipt, TaskResponsibility, TransferOwnerRequest, apply_accept,
    apply_agent_swap_for_person, apply_assign, apply_claim_available, apply_claim_execution,
    apply_mark_pending, apply_release_execution, apply_transfer_propose,
};
use serde_json::{Value, json};
use tokio_postgres::Transaction;

fn invalid() -> PgError {
    PgError::Protocol("invalid responsibility request".into())
}

pub(crate) fn map_core(err: awr_core::Error) -> PgError {
    match err {
        awr_core::Error::RevisionConflict { .. } => PgError::PreconditionsChanged,
        awr_core::Error::ClaimConflict(_) => PgError::ClaimHeld,
        awr_core::Error::RuleViolation(_) => PgError::Forbidden,
        awr_core::Error::NotFound(_) => PgError::Protocol(format!("not found: {err}")),
        awr_core::Error::InvalidInput(m) => PgError::Protocol(m),
        other => PgError::Protocol(other.to_string()),
    }
}

fn event_type_str(op: ResponsibilityEventType) -> &'static str {
    match op {
        ResponsibilityEventType::Assigned => "assigned",
        ResponsibilityEventType::Accepted => "accepted",
        ResponsibilityEventType::AvailableClaimed => "available_claimed",
        ResponsibilityEventType::ExecutionClaimed => "execution_claimed",
        ResponsibilityEventType::ExecutionReleased => "execution_released",
        ResponsibilityEventType::OwnerTransferProposed => "owner_transfer_proposed",
        ResponsibilityEventType::OwnerTransferAccepted => "owner_transfer_accepted",
        ResponsibilityEventType::OwnerTransferRejected => "owner_transfer_rejected",
        ResponsibilityEventType::CollaboratorsUpdated => "collaborators_updated",
        ResponsibilityEventType::ReviewerUpdated => "reviewer_updated",
        ResponsibilityEventType::AgentBound => "agent_bound",
        ResponsibilityEventType::AgentUnbound => "agent_unbound",
        ResponsibilityEventType::PendingMarked => "pending_marked",
        ResponsibilityEventType::PendingCleared => "pending_cleared",
    }
}

fn pending_kind_str(kind: ResponsibilityPendingKind) -> &'static str {
    match kind {
        ResponsibilityPendingKind::Departure => "departure",
        ResponsibilityPendingKind::Disabled => "disabled",
        ResponsibilityPendingKind::NoAcceptor => "no_acceptor",
        ResponsibilityPendingKind::LegacyIdentityMigration => "legacy_identity_migration",
    }
}

fn request_hash(
    tenant: &str,
    project: &str,
    work_id: &str,
    operation: &str,
    actor: Option<&str>,
    input: Value,
) -> PgResult<String> {
    awr_team::request_hash(&json!({
        "protocol": "awr.responsibility.request.v1",
        "tenant": tenant,
        "project": project,
        "work": work_id,
        "operation": operation,
        "actor": actor,
        "input": input,
    }))
    .map_err(|error| PgError::Protocol(error.to_string()))
}

pub struct ResponsibilityStore {
    pool: crate::PgPool,
}

impl ResponsibilityStore {
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

    pub async fn ensure_person(
        &self,
        tenant: &str,
        project: &str,
        person_id: &str,
        display_name: &str,
    ) -> PgResult<()> {
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_workstream_scope(&tx, tenant, project).await?;
        ensure_person_tx(&tx, tenant, project, person_id, display_name).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn bind_person_agent(
        &self,
        tenant: &str,
        project: &str,
        binding: &PersonAgentBinding,
    ) -> PgResult<()> {
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_workstream_scope(&tx, tenant, project).await?;
        crate::agent_authorization::lock_project(&tx, tenant, project).await?;
        let status = match binding.status {
            BindingStatus::Active => "active",
            BindingStatus::Disabled => "disabled",
        };
        ensure_person_tx(
            &tx,
            tenant,
            project,
            binding.person_id.as_str(),
            binding.person_id.as_str(),
        )
        .await?;
        tx.execute(
            "INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
             VALUES($1,$2,$3,$4,$5,$6)
             ON CONFLICT(tenant_id,project_id,id) DO UPDATE
             SET status=EXCLUDED.status, agent_id=EXCLUDED.agent_id, person_id=EXCLUDED.person_id",
            &[
                &tenant,
                &project,
                &binding.id,
                &binding.person_id.as_str(),
                &binding.agent_id,
                &status,
            ],
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn get(
        &self,
        tenant: &str,
        project: &str,
        work_id: &str,
    ) -> PgResult<TaskResponsibility> {
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_workstream_scope(&tx, tenant, project).await?;
        let task = current(&tx, tenant, project, work_id).await?;
        tx.commit().await?;
        Ok(task)
    }

    pub async fn assign(
        &self,
        tenant: &str,
        project: &str,
        work_id: &str,
        req: &AssignResponsibilityRequest,
    ) -> PgResult<(TaskResponsibility, ResponsibilityReceipt)> {
        self.apply(
            tenant,
            project,
            work_id,
            &req.request_key,
            ResponsibilityEventType::Assigned,
            request_hash(
                tenant,
                project,
                work_id,
                "assign",
                Some(req.authorized_by.as_str()),
                json!(req),
            )?,
            |before, bindings| {
                let _ = bindings;
                apply_assign(before, req).map_err(map_core)
            },
            Some(req.authorized_by.as_str()),
            json!({"owner": req.owner.as_ref().map(|p| p.as_str())}),
        )
        .await
    }

    pub async fn accept(
        &self,
        tenant: &str,
        project: &str,
        work_id: &str,
        req: &AcceptResponsibilityRequest,
    ) -> PgResult<(TaskResponsibility, ResponsibilityReceipt)> {
        self.apply(
            tenant,
            project,
            work_id,
            &req.request_key,
            ResponsibilityEventType::Accepted,
            request_hash(
                tenant,
                project,
                work_id,
                "accept",
                Some(req.acceptor.as_str()),
                json!(req),
            )?,
            |before, _| apply_accept(before, req).map_err(map_core),
            Some(req.acceptor.as_str()),
            json!({"acceptor": req.acceptor.as_str()}),
        )
        .await
    }

    /// Persistence kernel only: authenticated task admission and leases are separate.
    pub async fn claim_available(
        &self,
        tenant: &str,
        project: &str,
        work_id: &str,
        req: &ClaimAvailableRequest,
    ) -> PgResult<(TaskResponsibility, ResponsibilityReceipt)> {
        self.apply(
            tenant,
            project,
            work_id,
            &req.request_key,
            ResponsibilityEventType::AvailableClaimed,
            request_hash(
                tenant,
                project,
                work_id,
                "claim_available",
                Some(req.claimant.as_str()),
                json!(req),
            )?,
            |before, _| apply_claim_available(before, req).map_err(map_core),
            Some(req.claimant.as_str()),
            json!({"claimant": req.claimant, "execution_unchanged": true}),
        )
        .await
    }

    pub async fn claim_execution(
        &self,
        tenant: &str,
        project: &str,
        work_id: &str,
        req: &ClaimExecutionRequest,
    ) -> PgResult<(TaskResponsibility, ResponsibilityReceipt)> {
        self.apply(
            tenant,
            project,
            work_id,
            &req.request_key,
            ResponsibilityEventType::ExecutionClaimed,
            request_hash(
                tenant,
                project,
                work_id,
                "claim_execution",
                Some(req.executor.person_id().as_str()),
                json!(req),
            )?,
            |before, bindings| apply_claim_execution(before, req, bindings).map_err(map_core),
            Some(req.executor.person_id().as_str()),
            json!({
                "coordination_claim_id": req.coordination_claim_id,
                "owner_unchanged": true,
            }),
        )
        .await
    }

    pub async fn release_execution(
        &self,
        tenant: &str,
        project: &str,
        work_id: &str,
        request_key: &str,
        expected_version: u64,
        by_person: &PersonId,
    ) -> PgResult<(TaskResponsibility, ResponsibilityReceipt)> {
        self.apply(
            tenant,
            project,
            work_id,
            request_key,
            ResponsibilityEventType::ExecutionReleased,
            request_hash(tenant, project, work_id, "release_execution", Some(by_person.as_str()),
                json!({"request_key": request_key, "expected_version": expected_version, "by_person": by_person}))?,
            |before, _| {
                apply_release_execution(before, request_key, expected_version, by_person)
                    .map_err(map_core)
            },
            Some(by_person.as_str()),
            json!({}),
        )
        .await
    }

    pub async fn transfer_owner(
        &self,
        tenant: &str,
        project: &str,
        work_id: &str,
        req: &TransferOwnerRequest,
    ) -> PgResult<(TaskResponsibility, ResponsibilityReceipt)> {
        self.apply(
            tenant,
            project,
            work_id,
            &req.request_key,
            ResponsibilityEventType::OwnerTransferProposed,
            request_hash(
                tenant,
                project,
                work_id,
                "transfer_owner",
                Some(req.authorized_by.as_str()),
                json!(req),
            )?,
            |before, _| apply_transfer_propose(before, req).map_err(map_core),
            Some(req.authorized_by.as_str()),
            json!({
                "from": req.from_owner.as_str(),
                "to": req.to_owner.as_str(),
            }),
        )
        .await
    }

    pub async fn swap_agent(
        &self,
        tenant: &str,
        project: &str,
        work_id: &str,
        request_key: &str,
        expected_version: u64,
        person_id: &PersonId,
        new_executor: ExecutionInstance,
    ) -> PgResult<(TaskResponsibility, ResponsibilityReceipt)> {
        self.apply(
            tenant,
            project,
            work_id,
            request_key,
            ResponsibilityEventType::ExecutionClaimed,
            request_hash(
                tenant,
                project,
                work_id,
                "swap_agent",
                Some(person_id.as_str()),
                json!({"request_key": request_key, "expected_version": expected_version,
                    "person_id": person_id, "new_executor": new_executor}),
            )?,
            move |before, bindings| {
                apply_agent_swap_for_person(
                    before,
                    request_key,
                    expected_version,
                    person_id,
                    new_executor.clone(),
                    bindings,
                )
                .map_err(map_core)
            },
            Some(person_id.as_str()),
            json!({"agent_swap": true, "owner_unchanged": true}),
        )
        .await
    }

    pub async fn mark_pending(
        &self,
        tenant: &str,
        project: &str,
        work_id: &str,
        request_key: &str,
        expected_version: u64,
        pending: ResponsibilityPending,
    ) -> PgResult<(TaskResponsibility, ResponsibilityReceipt)> {
        let kind = pending_kind_str(pending.kind);
        let actor = pending.person_id.as_ref().map(|p| p.as_str().to_string());
        self.apply(
            tenant,
            project,
            work_id,
            request_key,
            ResponsibilityEventType::PendingMarked,
            request_hash(tenant, project, work_id, "mark_pending", actor.as_deref(),
                json!({"request_key": request_key, "expected_version": expected_version, "pending": pending}))?,
            move |before, _| {
                apply_mark_pending(before, request_key, expected_version, pending).map_err(map_core)
            },
            actor.as_deref(),
            json!({"kind": kind}),
        )
        .await
    }

    async fn apply<F>(
        &self,
        tenant: &str,
        project: &str,
        work_id: &str,
        request_key: &str,
        op: ResponsibilityEventType,
        request_hash: String,
        transition: F,
        actor: Option<&str>,
        payload: Value,
    ) -> PgResult<(TaskResponsibility, ResponsibilityReceipt)>
    where
        F: FnOnce(&TaskResponsibility, &[PersonAgentBinding]) -> PgResult<TaskResponsibility>,
    {
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_workstream_scope(&tx, tenant, project).await?;
        let result = apply_in_transaction(
            &tx,
            tenant,
            project,
            work_id,
            request_key,
            op,
            request_hash,
            transition,
            actor,
            payload,
        )
        .await?;
        tx.commit().await?;
        Ok(result)
    }
}

/// Reuse the caller's authenticated transaction so responsibility and leases
/// either commit together or roll back together.
pub(crate) async fn apply_in_transaction<F>(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work_id: &str,
    request_key: &str,
    op: ResponsibilityEventType,
    request_hash: String,
    transition: F,
    actor: Option<&str>,
    payload: Value,
) -> PgResult<(TaskResponsibility, ResponsibilityReceipt)>
where
    F: FnOnce(&TaskResponsibility, &[PersonAgentBinding]) -> PgResult<TaskResponsibility>,
{
    lock_transition_keys(tx, tenant, project, work_id, &[request_key.to_owned()]).await?;
    if let Some(receipt) =
        load_receipt(&tx, tenant, project, request_key, op, &request_hash).await?
    {
        if receipt.work_item_id != work_id {
            return Err(PgError::IdempotencyConflict);
        }
        let after = current(tx, tenant, project, work_id).await?;
        return Ok((after, receipt));
    }
    let before = current(tx, tenant, project, work_id).await?;
    let bindings = load_bindings_tx(&tx, tenant, project).await?;
    let after = transition(&before, &bindings)?;
    persist_task(&tx, tenant, project, &after).await?;
    let receipt = record_change(
        &tx,
        tenant,
        &before,
        &after,
        op,
        request_key,
        &request_hash,
        actor,
        payload,
    )
    .await?;
    Ok((after, receipt))
}

/// Acquire all request locks before the task lock, including when a caller needs
/// several responsibility transitions inside one authenticated transaction.
pub(crate) async fn lock_transition_keys(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work_id: &str,
    request_keys: &[String],
) -> PgResult<()> {
    // Receipt keys are project-wide, including requests for different tasks.
    // Lock the request before the task so key reuse produces a domain conflict,
    // rather than a late unique violation after writing a task projection.
    for request_key in request_keys {
        let request_lock =
            serde_json::to_string(&("responsibility.request", tenant, project, request_key))
                .map_err(|error| PgError::Protocol(error.to_string()))?;
        tx.execute(
            "SELECT pg_advisory_xact_lock(hashtext('awr.responsibility.request.v1'), hashtext($1))",
            &[&request_lock],
        )
        .await?;
    }
    // Serialize first-insert races. FOR UPDATE cannot lock a missing row, so two
    // creators would otherwise both observe "unassigned" and the later upsert
    // would erase the earlier owner.
    let lock_key = format!("{tenant}\u{1f}{project}\u{1f}{work_id}");
    tx.execute(
        "SELECT pg_advisory_xact_lock(hashtext($1)::bigint)",
        &[&lock_key],
    )
    .await?;
    Ok(())
}

async fn load_bindings_tx(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
) -> PgResult<Vec<PersonAgentBinding>> {
    let rows = tx
        .query(
            "SELECT id, person_id, agent_id, status, (EXTRACT(EPOCH FROM created_at)*1000)::bigint
             FROM awr_team.person_agent_bindings WHERE tenant_id=$1 AND project_id=$2",
            &[&tenant, &project],
        )
        .await?;
    let mut out = Vec::new();
    for row in rows {
        let status: String = row.get(3);
        out.push(PersonAgentBinding {
            id: row.get(0),
            person_id: PersonId::new(row.get::<_, String>(1)).map_err(map_core)?,
            agent_id: row.get(2),
            status: if status == "active" {
                BindingStatus::Active
            } else {
                BindingStatus::Disabled
            },
            created_at_ms: row.get(4),
        });
    }
    Ok(out)
}

/// An absent projection is the pool state. Reads do not create a person or
/// responsibility row and do not acquire mutation row locks.
pub(crate) async fn current(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work_id: &str,
) -> PgResult<TaskResponsibility> {
    Ok(load_task(tx, tenant, project, work_id)
        .await?
        .unwrap_or_else(|| TaskResponsibility::unassigned(project, work_id)))
}

async fn load_task(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work_id: &str,
) -> PgResult<Option<TaskResponsibility>> {
    let query = "SELECT owner_person_id, independent_reviewer_person_id, executor_kind, executor_person_id,
                    executor_agent_id, executor_binding_id, version, pending_kind, pending_person_id,
                    pending_legacy_ref, pending_transfer_request_key, pending_detail, personal_mode_default
             FROM awr_team.task_responsibilities
             WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3";
    let row = tx.query_opt(query, &[&tenant, &project, &work_id]).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    finish_task_tx(tx, tenant, project, work_id, row).await
}

async fn finish_task_tx(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work_id: &str,
    row: tokio_postgres::Row,
) -> PgResult<Option<TaskResponsibility>> {
    let collaborators = load_collaborators_tx(tx, tenant, project, work_id).await?;
    Ok(Some(row_to_task(project, work_id, row, collaborators)?))
}

fn row_to_task(
    project: &str,
    work_id: &str,
    row: tokio_postgres::Row,
    collaborators: Vec<PersonId>,
) -> PgResult<TaskResponsibility> {
    let owner: Option<String> = row.get(0);
    let reviewer: Option<String> = row.get(1);
    let exec_kind: Option<String> = row.get(2);
    let exec_person: Option<String> = row.get(3);
    let exec_agent: Option<String> = row.get(4);
    let exec_binding: Option<String> = row.get(5);
    let version: i64 = row.get(6);
    let pending_kind_s: Option<String> = row.get(7);
    let pending_person: Option<String> = row.get(8);
    let pending_legacy: Option<String> = row.get(9);
    let pending_xfer: Option<String> = row.get(10);
    let pending_detail: Option<String> = row.get(11);
    let personal: bool = row.get(12);
    let current_executor = match (exec_kind.as_deref(), exec_person) {
        (None, None) => None,
        (Some("person"), Some(p)) => Some(ExecutionInstance::Person {
            person_id: PersonId::new(p).map_err(map_core)?,
        }),
        (Some("agent_run"), Some(p)) => Some(ExecutionInstance::AgentRun {
            person_id: PersonId::new(p).map_err(map_core)?,
            agent_id: exec_agent.ok_or_else(invalid)?,
            binding_id: exec_binding.ok_or_else(invalid)?,
        }),
        _ => return Err(invalid()),
    };
    let pending = match (pending_kind_s.as_deref(), pending_detail) {
        (None, None) => None,
        (Some(kind), Some(detail)) => Some(ResponsibilityPending {
            kind: match kind {
                "departure" => ResponsibilityPendingKind::Departure,
                "disabled" => ResponsibilityPendingKind::Disabled,
                "no_acceptor" => ResponsibilityPendingKind::NoAcceptor,
                "legacy_identity_migration" => ResponsibilityPendingKind::LegacyIdentityMigration,
                _ => return Err(invalid()),
            },
            person_id: pending_person
                .map(PersonId::new)
                .transpose()
                .map_err(map_core)?,
            legacy_ref: pending_legacy,
            transfer_request_key: pending_xfer,
            detail,
        }),
        _ => return Err(invalid()),
    };
    Ok(TaskResponsibility {
        project_id: project.into(),
        work_item_id: work_id.into(),
        owner: owner.map(PersonId::new).transpose().map_err(map_core)?,
        collaborators,
        current_executor,
        independent_reviewer: reviewer.map(PersonId::new).transpose().map_err(map_core)?,
        version: version as u64,
        pending,
        personal_mode_default: personal,
    })
}

async fn load_collaborators_tx(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work_id: &str,
) -> PgResult<Vec<PersonId>> {
    let rows = tx
        .query(
            "SELECT person_id FROM awr_team.task_collaborators
             WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 ORDER BY person_id",
            &[&tenant, &project, &work_id],
        )
        .await?;
    rows.into_iter()
        .map(|r| PersonId::new(r.get::<_, String>(0)).map_err(map_core))
        .collect()
}

async fn persist_task(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    task: &TaskResponsibility,
) -> PgResult<()> {
    if let Some(owner) = &task.owner {
        ensure_person_tx(tx, tenant, project, owner.as_str(), owner.as_str()).await?;
    }
    if let Some(reviewer) = &task.independent_reviewer {
        ensure_person_tx(tx, tenant, project, reviewer.as_str(), reviewer.as_str()).await?;
    }
    for c in &task.collaborators {
        ensure_person_tx(tx, tenant, project, c.as_str(), c.as_str()).await?;
    }
    if let Some(exec) = &task.current_executor {
        ensure_person_tx(
            tx,
            tenant,
            project,
            exec.person_id().as_str(),
            exec.person_id().as_str(),
        )
        .await?;
    }
    let (exec_kind, exec_person, exec_agent, exec_binding): (
        Option<&str>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = match &task.current_executor {
        None => (None, None, None, None),
        Some(ExecutionInstance::Person { person_id }) => {
            (Some("person"), Some(person_id.as_str().into()), None, None)
        }
        Some(ExecutionInstance::AgentRun {
            person_id,
            agent_id,
            binding_id,
        }) => (
            Some("agent_run"),
            Some(person_id.as_str().into()),
            Some(agent_id.clone()),
            Some(binding_id.clone()),
        ),
    };
    let (p_kind, p_person, p_legacy, p_xfer, p_detail): (
        Option<&str>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = match &task.pending {
        None => (None, None, None, None, None),
        Some(p) => (
            Some(pending_kind_str(p.kind)),
            p.person_id.as_ref().map(|x| x.as_str().into()),
            p.legacy_ref.clone(),
            p.transfer_request_key.clone(),
            Some(p.detail.clone()),
        ),
    };
    let version = task.version as i64;
    let written = tx.execute(
        "INSERT INTO awr_team.task_responsibilities(
            tenant_id,project_id,work_id,owner_person_id,independent_reviewer_person_id,
            executor_kind,executor_person_id,executor_agent_id,executor_binding_id,version,
            pending_kind,pending_person_id,pending_legacy_ref,pending_transfer_request_key,pending_detail,
            personal_mode_default,updated_at)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,clock_timestamp())
         ON CONFLICT(tenant_id,project_id,work_id) DO UPDATE SET
            owner_person_id=EXCLUDED.owner_person_id,
            independent_reviewer_person_id=EXCLUDED.independent_reviewer_person_id,
            executor_kind=EXCLUDED.executor_kind,
            executor_person_id=EXCLUDED.executor_person_id,
            executor_agent_id=EXCLUDED.executor_agent_id,
            executor_binding_id=EXCLUDED.executor_binding_id,
            version=EXCLUDED.version,
            pending_kind=EXCLUDED.pending_kind,
            pending_person_id=EXCLUDED.pending_person_id,
            pending_legacy_ref=EXCLUDED.pending_legacy_ref,
            pending_transfer_request_key=EXCLUDED.pending_transfer_request_key,
            pending_detail=EXCLUDED.pending_detail,
            personal_mode_default=EXCLUDED.personal_mode_default,
            updated_at=clock_timestamp()
         WHERE awr_team.task_responsibilities.version = EXCLUDED.version - 1",
        &[
            &tenant,
            &project,
            &task.work_item_id,
            &task.owner.as_ref().map(|p| p.as_str()),
            &task.independent_reviewer.as_ref().map(|p| p.as_str()),
            &exec_kind,
            &exec_person,
            &exec_agent,
            &exec_binding,
            &version,
            &p_kind,
            &p_person,
            &p_legacy,
            &p_xfer,
            &p_detail,
            &task.personal_mode_default,
        ],
    )
    .await?;
    if written != 1 {
        return Err(PgError::PreconditionsChanged);
    }
    tx.execute(
        "DELETE FROM awr_team.task_collaborators WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3",
        &[&tenant, &project, &task.work_item_id],
    )
    .await?;
    for c in &task.collaborators {
        tx.execute(
            "INSERT INTO awr_team.task_collaborators(tenant_id,project_id,work_id,person_id)
             VALUES($1,$2,$3,$4)",
            &[&tenant, &project, &task.work_item_id, &c.as_str()],
        )
        .await?;
    }
    Ok(())
}

async fn ensure_person_tx(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    person_id: &str,
    display_name: &str,
) -> PgResult<()> {
    tx.execute(
        "INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
         VALUES($1,$2,$3,$4,'active') ON CONFLICT DO NOTHING",
        &[&tenant, &project, &person_id, &display_name],
    )
    .await?;
    Ok(())
}

async fn load_receipt(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    request_key: &str,
    op: ResponsibilityEventType,
    request_hash: &str,
) -> PgResult<Option<ResponsibilityReceipt>> {
    let row = tx
        .query_opt(
            "SELECT event_id, work_id, op, version_before, version_after, request_hash
             FROM awr_team.responsibility_receipts
             WHERE tenant_id=$1 AND project_id=$2 AND request_key=$3",
            &[&tenant, &project, &request_key],
        )
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let existing_op: String = row.get(2);
    let existing_hash: Option<String> = row.get(5);
    if existing_op != event_type_str(op) || existing_hash.as_deref() != Some(request_hash) {
        return Err(PgError::IdempotencyConflict);
    }
    let event_id: String = row.get(0);
    let work_id: String = row.get(1);
    let vb: i64 = row.get(3);
    let va: i64 = row.get(4);
    Ok(Some(ResponsibilityReceipt {
        request_key: request_key.into(),
        event_id: event_id.parse().map_err(|_| invalid())?,
        project_id: project.into(),
        work_item_id: work_id,
        op,
        version_before: vb as u64,
        version_after: va as u64,
        replayed: true,
    }))
}

async fn record_change(
    tx: &Transaction<'_>,
    tenant: &str,
    before: &TaskResponsibility,
    after: &TaskResponsibility,
    op: ResponsibilityEventType,
    request_key: &str,
    request_hash: &str,
    actor: Option<&str>,
    payload: Value,
) -> PgResult<ResponsibilityReceipt> {
    let event_id = new_id();
    let vb = before.version as i64;
    let va = after.version as i64;
    let op_s = event_type_str(op);
    tx.execute(
        "INSERT INTO awr_team.responsibility_events(
            tenant_id,project_id,id,work_id,event_type,version_before,version_after,actor_person_id,payload_json)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)",
        &[
            &tenant,
            &after.project_id,
            &event_id,
            &after.work_item_id,
            &op_s,
            &vb,
            &va,
            &actor,
            &payload,
        ],
    )
    .await?;
    tx.execute(
        "INSERT INTO awr_team.responsibility_receipts(
            tenant_id,project_id,request_key,event_id,work_id,op,version_before,version_after,request_hash)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)",
        &[
            &tenant,
            &after.project_id,
            &request_key,
            &event_id,
            &after.work_item_id,
            &op_s,
            &vb,
            &va,
            &request_hash,
        ],
    )
    .await?;
    Ok(ResponsibilityReceipt {
        request_key: request_key.into(),
        event_id: event_id.parse().map_err(|_| invalid())?,
        project_id: after.project_id.clone(),
        work_item_id: after.work_item_id.clone(),
        op,
        version_before: before.version,
        version_after: after.version,
        replayed: false,
    })
}
