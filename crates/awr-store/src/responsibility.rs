//! SQLite persistence for person responsibility and execution-instance claims.
//! Coordination `claims` rows remain separate; execution claim never steals ownership.
use crate::{Store, db_error};
use awr_core::{
    AcceptResponsibilityRequest, AssignResponsibilityRequest, BindingStatus, ClaimExecutionRequest,
    Error, ExecutionInstance, Id, PersonAgentBinding, PersonId, ResponsibilityEvent,
    ResponsibilityEventType, ResponsibilityPending, ResponsibilityPendingKind,
    ResponsibilityReceipt, Result, TaskResponsibility, TransferOwnerRequest, apply_accept,
    apply_agent_swap_for_person, apply_assign, apply_claim_execution, apply_mark_pending,
    apply_release_execution, apply_transfer_propose, now_millis,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::json;

fn person_row(id: Option<String>) -> Result<Option<PersonId>> {
    id.map(PersonId::new).transpose()
}

fn pending_kind(value: &str) -> Result<ResponsibilityPendingKind> {
    match value {
        "departure" => Ok(ResponsibilityPendingKind::Departure),
        "disabled" => Ok(ResponsibilityPendingKind::Disabled),
        "no_acceptor" => Ok(ResponsibilityPendingKind::NoAcceptor),
        "legacy_identity_migration" => Ok(ResponsibilityPendingKind::LegacyIdentityMigration),
        _ => Err(Error::Storage("unknown pending kind".into())),
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

fn load_collaborators(conn: &Connection, project: &str, work: &str) -> Result<Vec<PersonId>> {
    conn.prepare(
        "SELECT person_id FROM task_collaborators WHERE project_id=?1 AND work_item_id=?2 ORDER BY person_id",
    )
    .map_err(db_error)?
    .query_map(params![project, work], |r| r.get::<_, String>(0))
    .map_err(db_error)?
    .map(|r| PersonId::new(r.map_err(db_error)?))
    .collect()
}

fn load_task(conn: &Connection, project: &str, work: &str) -> Result<Option<TaskResponsibility>> {
    let row = conn
        .query_row(
            "SELECT owner_person_id, independent_reviewer_person_id, executor_kind, executor_person_id,
                    executor_agent_id, executor_binding_id, version, pending_kind, pending_person_id,
                    pending_legacy_ref, pending_transfer_request_key, pending_detail, personal_mode_default
             FROM task_responsibilities WHERE project_id=?1 AND work_item_id=?2",
            params![project, work],
            |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    r.get::<_, Option<String>>(8)?,
                    r.get::<_, Option<String>>(9)?,
                    r.get::<_, Option<String>>(10)?,
                    r.get::<_, Option<String>>(11)?,
                    r.get::<_, i64>(12)?,
                ))
            },
        )
        .optional()
        .map_err(db_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let (
        owner,
        reviewer,
        exec_kind,
        exec_person,
        exec_agent,
        exec_binding,
        version,
        pending_kind_s,
        pending_person,
        pending_legacy,
        pending_xfer,
        pending_detail,
        personal,
    ) = row;
    let current_executor = match (exec_kind.as_deref(), exec_person) {
        (None, None) => None,
        (Some("person"), Some(p)) => Some(ExecutionInstance::Person {
            person_id: PersonId::new(p)?,
        }),
        (Some("agent_run"), Some(p)) => Some(ExecutionInstance::AgentRun {
            person_id: PersonId::new(p)?,
            agent_id: exec_agent.ok_or_else(|| Error::Storage("agent_run missing agent".into()))?,
            binding_id: exec_binding
                .ok_or_else(|| Error::Storage("agent_run missing binding".into()))?,
        }),
        _ => {
            return Err(Error::Storage(
                "task_responsibilities executor columns inconsistent".into(),
            ));
        }
    };
    let pending = match (pending_kind_s, pending_detail) {
        (None, None) => None,
        (Some(kind), Some(detail)) => Some(ResponsibilityPending {
            kind: pending_kind(&kind)?,
            person_id: person_row(pending_person)?,
            legacy_ref: pending_legacy,
            transfer_request_key: pending_xfer,
            detail,
        }),
        _ => {
            return Err(Error::Storage(
                "task_responsibilities pending columns inconsistent".into(),
            ));
        }
    };
    Ok(Some(TaskResponsibility {
        project_id: project.into(),
        work_item_id: work.into(),
        owner: person_row(owner)?,
        collaborators: load_collaborators(conn, project, work)?,
        current_executor,
        independent_reviewer: person_row(reviewer)?,
        version: version as u64,
        pending,
        personal_mode_default: personal != 0,
    }))
}

fn begin_immediate(conn: &mut Connection) -> Result<rusqlite::Transaction<'_>> {
    conn.transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error)
}

fn load_bindings(conn: &Connection, project: &str) -> Result<Vec<PersonAgentBinding>> {
    conn.prepare(
        "SELECT id, person_id, agent_id, status, created_at FROM person_agent_bindings WHERE project_id=?1",
    )
    .map_err(db_error)?
    .query_map(params![project], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, i64>(4)?,
        ))
    })
    .map_err(db_error)?
    .map(|row| {
        let (id, person_id, agent_id, status, created_at) = row.map_err(db_error)?;
        Ok(PersonAgentBinding {
            id,
            person_id: PersonId::new(person_id)?,
            agent_id,
            status: if status == "active" {
                BindingStatus::Active
            } else {
                BindingStatus::Disabled
            },
            created_at_ms: created_at,
        })
    })
    .collect()
}

fn ensure_person(conn: &Connection, project: &str, person: &PersonId, now: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO persons(project_id,id,display_name,status,created_at) VALUES(?1,?2,?2,'active',?3)
         ON CONFLICT(project_id,id) DO NOTHING",
        params![project, person.as_str(), now],
    )
    .map_err(db_error)?;
    Ok(())
}

fn persist_task(conn: &Connection, task: &TaskResponsibility, now: i64) -> Result<()> {
    let (exec_kind, exec_person, exec_agent, exec_binding) = match &task.current_executor {
        None => (None, None, None, None),
        Some(ExecutionInstance::Person { person_id }) => (
            Some("person"),
            Some(person_id.as_str().to_string()),
            None,
            None,
        ),
        Some(ExecutionInstance::AgentRun {
            person_id,
            agent_id,
            binding_id,
        }) => (
            Some("agent_run"),
            Some(person_id.as_str().to_string()),
            Some(agent_id.clone()),
            Some(binding_id.clone()),
        ),
    };
    let (p_kind, p_person, p_legacy, p_xfer, p_detail) = match &task.pending {
        None => (None, None, None, None, None),
        Some(p) => (
            Some(pending_kind_str(p.kind)),
            p.person_id.as_ref().map(|x| x.as_str().to_string()),
            p.legacy_ref.clone(),
            p.transfer_request_key.clone(),
            Some(p.detail.clone()),
        ),
    };
    if let Some(owner) = &task.owner {
        ensure_person(conn, &task.project_id, owner, now)?;
    }
    if let Some(reviewer) = &task.independent_reviewer {
        ensure_person(conn, &task.project_id, reviewer, now)?;
    }
    for c in &task.collaborators {
        ensure_person(conn, &task.project_id, c, now)?;
    }
    if let Some(exec) = &task.current_executor {
        ensure_person(conn, &task.project_id, exec.person_id(), now)?;
    }
    let written = conn.execute(
        "INSERT INTO task_responsibilities(
            project_id,work_item_id,owner_person_id,independent_reviewer_person_id,
            executor_kind,executor_person_id,executor_agent_id,executor_binding_id,version,
            pending_kind,pending_person_id,pending_legacy_ref,pending_transfer_request_key,pending_detail,
            personal_mode_default,updated_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)
         ON CONFLICT(project_id,work_item_id) DO UPDATE SET
            owner_person_id=excluded.owner_person_id,
            independent_reviewer_person_id=excluded.independent_reviewer_person_id,
            executor_kind=excluded.executor_kind,
            executor_person_id=excluded.executor_person_id,
            executor_agent_id=excluded.executor_agent_id,
            executor_binding_id=excluded.executor_binding_id,
            version=excluded.version,
            pending_kind=excluded.pending_kind,
            pending_person_id=excluded.pending_person_id,
            pending_legacy_ref=excluded.pending_legacy_ref,
            pending_transfer_request_key=excluded.pending_transfer_request_key,
            pending_detail=excluded.pending_detail,
            personal_mode_default=excluded.personal_mode_default,
            updated_at=excluded.updated_at
         WHERE version = excluded.version - 1",
        params![
            task.project_id,
            task.work_item_id,
            task.owner.as_ref().map(|p| p.as_str()),
            task.independent_reviewer.as_ref().map(|p| p.as_str()),
            exec_kind,
            exec_person,
            exec_agent,
            exec_binding,
            task.version as i64,
            p_kind,
            p_person,
            p_legacy,
            p_xfer,
            p_detail,
            if task.personal_mode_default { 1 } else { 0 },
            now
        ],
    )
    .map_err(db_error)?;
    if written != 1 {
        let actual: i64 = conn
            .query_row(
                "SELECT version FROM task_responsibilities WHERE project_id=?1 AND work_item_id=?2",
                params![task.project_id, task.work_item_id],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        return Err(Error::RevisionConflict {
            expected: task.version.saturating_sub(1),
            actual: actual as u64,
        });
    }
    conn.execute(
        "DELETE FROM task_collaborators WHERE project_id=?1 AND work_item_id=?2",
        params![task.project_id, task.work_item_id],
    )
    .map_err(db_error)?;
    for c in &task.collaborators {
        conn.execute(
            "INSERT INTO task_collaborators(project_id,work_item_id,person_id) VALUES(?1,?2,?3)",
            params![task.project_id, task.work_item_id, c.as_str()],
        )
        .map_err(db_error)?;
    }
    Ok(())
}

fn record_change(
    conn: &Connection,
    before: &TaskResponsibility,
    after: &TaskResponsibility,
    op: ResponsibilityEventType,
    request_key: &str,
    actor: Option<&PersonId>,
    payload: serde_json::Value,
    now: i64,
) -> Result<ResponsibilityReceipt> {
    if let Some(existing) = conn
        .query_row(
            "SELECT event_id,work_item_id,op,version_before,version_after FROM responsibility_receipts
             WHERE project_id=?1 AND request_key=?2",
            params![after.project_id, request_key],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()
        .map_err(db_error)?
    {
        let (event_id, work, existing_op, vb, va) = existing;
        if work != after.work_item_id || existing_op != event_type_str(op) {
            return Err(Error::InvalidInput(
                "idempotent receipt request_key reused with a different op/task".into(),
            ));
        }
        return Ok(ResponsibilityReceipt {
            request_key: request_key.into(),
            event_id: event_id.parse().map_err(|_| {
                Error::Storage("stored responsibility event id is not a ulid".into())
            })?,
            project_id: after.project_id.clone(),
            work_item_id: work,
            op,
            version_before: vb as u64,
            version_after: va as u64,
            replayed: true,
        });
    }
    let event_id = Id::new();
    conn.execute(
        "INSERT INTO responsibility_events(id,project_id,work_item_id,event_type,version_before,version_after,actor_person_id,payload_json,created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            event_id.to_string(),
            after.project_id,
            after.work_item_id,
            event_type_str(op),
            before.version as i64,
            after.version as i64,
            actor.map(|p| p.as_str()),
            payload.to_string(),
            now
        ],
    )
    .map_err(db_error)?;
    conn.execute(
        "INSERT INTO responsibility_receipts(project_id,request_key,event_id,work_item_id,op,version_before,version_after,created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            after.project_id,
            request_key,
            event_id.to_string(),
            after.work_item_id,
            event_type_str(op),
            before.version as i64,
            after.version as i64,
            now
        ],
    )
    .map_err(db_error)?;
    Ok(ResponsibilityReceipt {
        request_key: request_key.into(),
        event_id,
        project_id: after.project_id.clone(),
        work_item_id: after.work_item_id.clone(),
        op,
        version_before: before.version,
        version_after: after.version,
        replayed: false,
    })
}

impl Store {
    pub fn ensure_person(
        &mut self,
        project: Id,
        person: &PersonId,
        display_name: &str,
    ) -> Result<()> {
        let now = now_millis()?;
        let project = project.to_string();
        self.conn
            .execute(
                "INSERT INTO persons(project_id,id,display_name,status,created_at) VALUES(?1,?2,?3,'active',?4)
                 ON CONFLICT(project_id,id) DO UPDATE SET display_name=excluded.display_name",
                params![project, person.as_str(), display_name, now],
            )
            .map_err(db_error)?;
        Ok(())
    }

    pub fn bind_person_agent(&mut self, project: Id, binding: &PersonAgentBinding) -> Result<()> {
        let project = project.to_string();
        ensure_person(
            &self.conn,
            &project,
            &binding.person_id,
            binding.created_at_ms,
        )?;
        let status = match binding.status {
            BindingStatus::Active => "active",
            BindingStatus::Disabled => "disabled",
        };
        self.conn
            .execute(
                "INSERT INTO person_agent_bindings(project_id,id,person_id,agent_id,status,created_at)
                 VALUES(?1,?2,?3,?4,?5,?6)
                 ON CONFLICT(project_id,id) DO UPDATE SET status=excluded.status,agent_id=excluded.agent_id,person_id=excluded.person_id",
                params![
                    project,
                    binding.id,
                    binding.person_id.as_str(),
                    binding.agent_id,
                    status,
                    binding.created_at_ms
                ],
            )
            .map_err(db_error)?;
        Ok(())
    }

    pub fn task_responsibility(
        &self,
        project: Id,
        work_item_id: &str,
    ) -> Result<TaskResponsibility> {
        let project = project.to_string();
        Ok(load_task(&self.conn, &project, work_item_id)?
            .unwrap_or_else(|| TaskResponsibility::unassigned(project, work_item_id)))
    }

    pub fn responsibility_events(
        &self,
        project: Id,
        work_item_id: &str,
    ) -> Result<Vec<ResponsibilityEvent>> {
        let project = project.to_string();
        self.conn
            .prepare(
                "SELECT id,event_type,version_before,version_after,actor_person_id,payload_json,created_at
                 FROM responsibility_events WHERE project_id=?1 AND work_item_id=?2 ORDER BY created_at,id",
            )
            .map_err(db_error)?
            .query_map(params![project, work_item_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, i64>(6)?,
                ))
            })
            .map_err(db_error)?
            .map(|row| {
                let (id, event_type, vb, va, actor, payload, created) = row.map_err(db_error)?;
                Ok(ResponsibilityEvent {
                    id: id.parse().map_err(|_| {
                        Error::Storage("responsibility event id is not a ulid".into())
                    })?,
                    project_id: project.clone(),
                    work_item_id: work_item_id.into(),
                    event_type: parse_event_type(&event_type)?,
                    version_before: vb as u64,
                    version_after: va as u64,
                    actor_person_id: person_row(actor)?,
                    payload: serde_json::from_str(&payload)?,
                    created_at_ms: created,
                })
            })
            .collect()
    }

    pub fn assign_responsibility(
        &mut self,
        project: Id,
        work_item_id: &str,
        req: &AssignResponsibilityRequest,
    ) -> Result<(TaskResponsibility, ResponsibilityReceipt)> {
        let project_s = project.to_string();
        {
            let tx = begin_immediate(&mut self.conn)?;
            let now = now_millis()?;
            let before = load_task(&tx, &project_s, work_item_id)?
                .unwrap_or_else(|| TaskResponsibility::unassigned(&project_s, work_item_id));
            if let Some(receipt) = replay_receipt(
                &tx,
                &project_s,
                work_item_id,
                &req.request_key,
                ResponsibilityEventType::Assigned,
            )? {
                let after = load_task(&tx, &project_s, work_item_id)?.unwrap_or(before);
                tx.commit().map_err(db_error)?;
                return Ok((after, receipt));
            }
            let after = apply_assign(&before, req)?;
            persist_task(&tx, &after, now)?;
            let receipt = record_change(
                &tx,
                &before,
                &after,
                ResponsibilityEventType::Assigned,
                &req.request_key,
                Some(&req.authorized_by),
                json!({"owner": after.owner.as_ref().map(|p| p.as_str())}),
                now,
            )?;
            let out = (after, receipt);
            tx.commit().map_err(db_error)?;
            Ok(out)
        }
    }

    pub fn accept_responsibility(
        &mut self,
        project: Id,
        work_item_id: &str,
        req: &AcceptResponsibilityRequest,
    ) -> Result<(TaskResponsibility, ResponsibilityReceipt)> {
        let project_s = project.to_string();
        {
            let tx = begin_immediate(&mut self.conn)?;
            let now = now_millis()?;
            let before = load_task(&tx, &project_s, work_item_id)?
                .ok_or_else(|| Error::NotFound("task responsibility missing".into()))?;
            if let Some(receipt) = replay_receipt(
                &tx,
                &project_s,
                work_item_id,
                &req.request_key,
                ResponsibilityEventType::Accepted,
            )? {
                let after = load_task(&tx, &project_s, work_item_id)?.unwrap_or(before);
                tx.commit().map_err(db_error)?;
                return Ok((after, receipt));
            }
            let after = apply_accept(&before, req)?;
            persist_task(&tx, &after, now)?;
            let receipt = record_change(
                &tx,
                &before,
                &after,
                ResponsibilityEventType::Accepted,
                &req.request_key,
                Some(&req.acceptor),
                json!({"acceptor": req.acceptor.as_str()}),
                now,
            )?;
            let out = (after, receipt);
            tx.commit().map_err(db_error)?;
            Ok(out)
        }
    }

    pub fn claim_execution_responsibility(
        &mut self,
        project: Id,
        work_item_id: &str,
        req: &ClaimExecutionRequest,
    ) -> Result<(TaskResponsibility, ResponsibilityReceipt)> {
        let project_s = project.to_string();
        {
            let tx = begin_immediate(&mut self.conn)?;
            let now = now_millis()?;
            let before = load_task(&tx, &project_s, work_item_id)?
                .unwrap_or_else(|| TaskResponsibility::unassigned(&project_s, work_item_id));
            if let Some(receipt) = replay_receipt(
                &tx,
                &project_s,
                work_item_id,
                &req.request_key,
                ResponsibilityEventType::ExecutionClaimed,
            )? {
                let after = load_task(&tx, &project_s, work_item_id)?.unwrap_or(before);
                tx.commit().map_err(db_error)?;
                return Ok((after, receipt));
            }
            let bindings = load_bindings(&tx, &project_s)?;
            let after = apply_claim_execution(&before, req, &bindings)?;
            persist_task(&tx, &after, now)?;
            let receipt = record_change(
                &tx,
                &before,
                &after,
                ResponsibilityEventType::ExecutionClaimed,
                &req.request_key,
                Some(req.executor.person_id()),
                json!({
                    "coordination_claim_id": req.coordination_claim_id,
                    "owner_unchanged": before.owner == after.owner,
                }),
                now,
            )?;
            let out = (after, receipt);
            tx.commit().map_err(db_error)?;
            Ok(out)
        }
    }

    pub fn release_execution_responsibility(
        &mut self,
        project: Id,
        work_item_id: &str,
        request_key: &str,
        expected_version: u64,
        by_person: &PersonId,
    ) -> Result<(TaskResponsibility, ResponsibilityReceipt)> {
        let project_s = project.to_string();
        {
            let tx = begin_immediate(&mut self.conn)?;
            let now = now_millis()?;
            let before = load_task(&tx, &project_s, work_item_id)?
                .ok_or_else(|| Error::NotFound("task responsibility missing".into()))?;
            if let Some(receipt) = replay_receipt(
                &tx,
                &project_s,
                work_item_id,
                request_key,
                ResponsibilityEventType::ExecutionReleased,
            )? {
                let after = load_task(&tx, &project_s, work_item_id)?.unwrap_or(before);
                tx.commit().map_err(db_error)?;
                return Ok((after, receipt));
            }
            let after = apply_release_execution(&before, request_key, expected_version, by_person)?;
            persist_task(&tx, &after, now)?;
            let receipt = record_change(
                &tx,
                &before,
                &after,
                ResponsibilityEventType::ExecutionReleased,
                request_key,
                Some(by_person),
                json!({}),
                now,
            )?;
            let out = (after, receipt);
            tx.commit().map_err(db_error)?;
            Ok(out)
        }
    }

    pub fn transfer_owner_responsibility(
        &mut self,
        project: Id,
        work_item_id: &str,
        req: &TransferOwnerRequest,
    ) -> Result<(TaskResponsibility, ResponsibilityReceipt)> {
        let project_s = project.to_string();
        {
            let tx = begin_immediate(&mut self.conn)?;
            let now = now_millis()?;
            let before = load_task(&tx, &project_s, work_item_id)?
                .ok_or_else(|| Error::NotFound("task responsibility missing".into()))?;
            if let Some(receipt) = replay_receipt(
                &tx,
                &project_s,
                work_item_id,
                &req.request_key,
                ResponsibilityEventType::OwnerTransferProposed,
            )? {
                let after = load_task(&tx, &project_s, work_item_id)?.unwrap_or(before);
                tx.commit().map_err(db_error)?;
                return Ok((after, receipt));
            }
            let after = apply_transfer_propose(&before, req)?;
            persist_task(&tx, &after, now)?;
            let receipt = record_change(
                &tx,
                &before,
                &after,
                ResponsibilityEventType::OwnerTransferProposed,
                &req.request_key,
                Some(&req.authorized_by),
                json!({
                    "from": req.from_owner.as_str(),
                    "to": req.to_owner.as_str(),
                }),
                now,
            )?;
            let out = (after, receipt);
            tx.commit().map_err(db_error)?;
            Ok(out)
        }
    }

    pub fn swap_agent_for_person(
        &mut self,
        project: Id,
        work_item_id: &str,
        request_key: &str,
        expected_version: u64,
        person_id: &PersonId,
        new_executor: ExecutionInstance,
    ) -> Result<(TaskResponsibility, ResponsibilityReceipt)> {
        let project_s = project.to_string();
        {
            let tx = begin_immediate(&mut self.conn)?;
            let now = now_millis()?;
            let before = load_task(&tx, &project_s, work_item_id)?
                .ok_or_else(|| Error::NotFound("task responsibility missing".into()))?;
            if let Some(receipt) = replay_receipt(
                &tx,
                &project_s,
                work_item_id,
                request_key,
                ResponsibilityEventType::ExecutionClaimed,
            )? {
                let after = load_task(&tx, &project_s, work_item_id)?.unwrap_or(before);
                tx.commit().map_err(db_error)?;
                return Ok((after, receipt));
            }
            let bindings = load_bindings(&tx, &project_s)?;
            let after = apply_agent_swap_for_person(
                &before,
                request_key,
                expected_version,
                person_id,
                new_executor,
                &bindings,
            )?;
            persist_task(&tx, &after, now)?;
            let receipt = record_change(
                &tx,
                &before,
                &after,
                ResponsibilityEventType::ExecutionClaimed,
                request_key,
                Some(person_id),
                json!({"agent_swap": true, "owner_unchanged": true}),
                now,
            )?;
            let out = (after, receipt);
            tx.commit().map_err(db_error)?;
            Ok(out)
        }
    }

    pub fn mark_responsibility_pending(
        &mut self,
        project: Id,
        work_item_id: &str,
        request_key: &str,
        expected_version: u64,
        pending: ResponsibilityPending,
    ) -> Result<(TaskResponsibility, ResponsibilityReceipt)> {
        let project_s = project.to_string();
        {
            let tx = begin_immediate(&mut self.conn)?;
            let now = now_millis()?;
            let before = load_task(&tx, &project_s, work_item_id)?
                .ok_or_else(|| Error::NotFound("task responsibility missing".into()))?;
            if let Some(receipt) = replay_receipt(
                &tx,
                &project_s,
                work_item_id,
                request_key,
                ResponsibilityEventType::PendingMarked,
            )? {
                let after = load_task(&tx, &project_s, work_item_id)?.unwrap_or(before);
                tx.commit().map_err(db_error)?;
                return Ok((after, receipt));
            }
            let after = apply_mark_pending(&before, request_key, expected_version, pending)?;
            persist_task(&tx, &after, now)?;
            let receipt = record_change(
                &tx,
                &before,
                &after,
                ResponsibilityEventType::PendingMarked,
                request_key,
                after.pending.as_ref().and_then(|p| p.person_id.as_ref()),
                json!({"kind": pending_kind_str(after.pending.as_ref().unwrap().kind)}),
                now,
            )?;
            let out = (after, receipt);
            tx.commit().map_err(db_error)?;
            Ok(out)
        }
    }
}

fn parse_event_type(value: &str) -> Result<ResponsibilityEventType> {
    Ok(match value {
        "assigned" => ResponsibilityEventType::Assigned,
        "accepted" => ResponsibilityEventType::Accepted,
        "available_claimed" => ResponsibilityEventType::AvailableClaimed,
        "execution_claimed" => ResponsibilityEventType::ExecutionClaimed,
        "execution_released" => ResponsibilityEventType::ExecutionReleased,
        "owner_transfer_proposed" => ResponsibilityEventType::OwnerTransferProposed,
        "owner_transfer_accepted" => ResponsibilityEventType::OwnerTransferAccepted,
        "owner_transfer_rejected" => ResponsibilityEventType::OwnerTransferRejected,
        "collaborators_updated" => ResponsibilityEventType::CollaboratorsUpdated,
        "reviewer_updated" => ResponsibilityEventType::ReviewerUpdated,
        "agent_bound" => ResponsibilityEventType::AgentBound,
        "agent_unbound" => ResponsibilityEventType::AgentUnbound,
        "pending_marked" => ResponsibilityEventType::PendingMarked,
        "pending_cleared" => ResponsibilityEventType::PendingCleared,
        _ => return Err(Error::Storage("unknown responsibility event type".into())),
    })
}

fn replay_receipt(
    conn: &Connection,
    project: &str,
    work_item_id: &str,
    request_key: &str,
    op: ResponsibilityEventType,
) -> Result<Option<ResponsibilityReceipt>> {
    let row = conn
        .query_row(
            "SELECT event_id,work_item_id,op,version_before,version_after FROM responsibility_receipts
             WHERE project_id=?1 AND request_key=?2",
            params![project, request_key],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()
        .map_err(db_error)?;
    let Some((event_id, work, existing_op, vb, va)) = row else {
        return Ok(None);
    };
    if existing_op != event_type_str(op) {
        return Err(Error::InvalidInput(
            "idempotent receipt request_key reused with a different op".into(),
        ));
    }
    if work != work_item_id {
        return Err(Error::InvalidInput(
            "idempotent receipt request_key reused for a different work item".into(),
        ));
    }
    Ok(Some(ResponsibilityReceipt {
        request_key: request_key.into(),
        event_id: event_id
            .parse()
            .map_err(|_| Error::Storage("stored responsibility event id is not a ulid".into()))?,
        project_id: project.into(),
        work_item_id: work,
        op,
        version_before: vb as u64,
        version_after: va as u64,
        replayed: true,
    }))
}
