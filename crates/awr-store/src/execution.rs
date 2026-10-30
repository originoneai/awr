//! Typed execution operations share the immutable project event journal and revision lock.
use crate::{Store, db_error};
use awr_core::*;
use rusqlite::{Connection, OptionalExtension, params};

fn decode(value: String) -> Result<Execution> {
    serde_json::from_str(&value)
        .map_err(|_| Error::Storage("invalid execution journal record".into()))
}
pub(crate) fn at(conn: &Connection, project: Id, id: Id) -> Result<Execution> {
    let value: Option<String> = conn.query_row(
        "SELECT json_extract(payload_json,'$.execution') FROM events WHERE project_id=?1 AND event_type IN ('execution.registered','execution.starting','execution.running','execution.finished') AND json_extract(payload_json,'$.execution.id')=?2 ORDER BY project_revision DESC LIMIT 1",
        params![project.to_string(),id.to_string()], |r| r.get(0)).optional().map_err(db_error)?;
    decode(value.ok_or_else(|| Error::NotFound(format!("execution {id}")))?)
}
/// Every execution of a work (or of the project), each in its latest state.
pub(crate) fn executions_of(
    conn: &Connection,
    project: Id,
    work: Option<Id>,
) -> Result<Vec<Execution>> {
    let mut q = conn.prepare("SELECT json_extract(payload_json,'$.execution') FROM events WHERE project_id=?1 AND event_type='execution.registered' AND (?2 IS NULL OR work_item_id=?2) ORDER BY project_revision").map_err(db_error)?;
    let rows = q
        .query_map(
            params![project.to_string(), work.map(|v| v.to_string())],
            |r| r.get::<_, String>(0),
        )
        .map_err(db_error)?;
    let mut result = Vec::new();
    for row in rows {
        let initial = decode(row.map_err(db_error)?)?;
        result.push(at(conn, project, initial.id)?);
    }
    Ok(result)
}
pub(crate) fn key_at(conn: &Connection, project: Id, key: &str) -> Result<Option<Execution>> {
    let value: Option<String> = conn.query_row(
        "SELECT json_extract(payload_json,'$.execution') FROM events WHERE project_id=?1 AND event_type IN ('execution.registered','execution.starting','execution.running','execution.finished') AND json_extract(payload_json,'$.execution.intent.operation_key')=?2 ORDER BY project_revision DESC LIMIT 1",
        params![project.to_string(),key], |r| r.get(0)).optional().map_err(db_error)?;
    value.map(decode).transpose()
}
impl Store {
    /// Immutable report lookup also works after its originating work session ends.
    pub fn external_report_by_key(&self, project: Id, key: &str) -> Result<Option<Event>> {
        report_at(&self.conn, project, "$.report.request_key", key)
    }
    pub fn latest_external_report(&self, project: Id, execution: Id) -> Result<Option<Event>> {
        self.execution(project, execution)?;
        report_at(
            &self.conn,
            project,
            "$.execution_id",
            &execution.to_string(),
        )
    }
    pub fn report_external_execution(
        &mut self,
        project: Id,
        expected: Revision,
        report: ExternalExecutionReport,
    ) -> Result<(Event, bool)> {
        report.validate()?;
        let same = |event: Event| -> Result<(Event, bool)> {
            if event.payload["report"] != serde_json::to_value(&report)? {
                return Err(Error::SourceConflict(
                    "external report request key was already used for different content".into(),
                ));
            }
            Ok((event, false))
        };
        if let Some(event) = self.external_report_by_key(project, &report.request_key)? {
            return same(event);
        }
        let result=self.runtime_transaction_with_event(project,expected,EventDraft::new("execution.external_reported",&report.summary),|tx,_,event| {
            let execution=at(tx,project,report.execution_id)?;
            if execution.intent.executor!=ExecutorKind::External {
                return Err(Error::InvalidTransition("host reports require an external execution; managed supervisors retain exclusive ownership".into()));
            }
            event.work_item_id=Some(execution.work_item_id);
            event.session_id=Some(execution.session_id);
            event.branch_id=execution.branch_id;
            event.payload=serde_json::json!({"execution_id":execution.id,"report":report});
            Ok(())
        });
        match result {
            Ok((_, event)) => Ok((event, true)),
            Err(error @ Error::RevisionConflict { .. }) => {
                match self.external_report_by_key(project, &report.request_key)? {
                    Some(event) => same(event),
                    None => Err(error),
                }
            }
            Err(error) => Err(error),
        }
    }
    pub fn execution(&self, project: Id, id: Id) -> Result<Execution> {
        at(&self.conn, project, id)
    }
    pub fn execution_by_key(&self, project: Id, key: &str) -> Result<Option<Execution>> {
        key_at(&self.conn, project, key)
    }
    /// Full lightweight registry for a work item. No terminal records are silently truncated.
    pub fn executions(&self, project: Id, work: Option<Id>) -> Result<Vec<Execution>> {
        executions_of(&self.conn, project, work)
    }
    pub fn register_execution(
        &mut self,
        project: Id,
        expected: Revision,
        session: Id,
        intent: ExecutionIntent,
    ) -> Result<(Execution, Event)> {
        ensure_public_data(&intent)?;
        if intent.operation_key.trim().is_empty()
            || intent.operation_key.len() > 512
            || intent.purpose.trim().is_empty()
            || intent.purpose.len() > 8192
            || intent.cwd.is_empty()
            || intent.cwd.len() > 4096
            || intent.command.len() > 256
            || intent
                .command
                .iter()
                .any(|s| s.len() > 8192 || s.contains('\0'))
        {
            return Err(Error::InvalidInput(
                "invalid or oversized execution intent".into(),
            ));
        }
        match intent.executor {
            ExecutorKind::ManagedLocal
                if intent.command.is_empty()
                    || intent.command[0].trim().is_empty()
                    || intent.external_reference.is_some() =>
            {
                return Err(Error::InvalidInput(
                    "managed execution requires an argument vector and no external reference"
                        .into(),
                ));
            }
            ExecutorKind::External
                if !intent.command.is_empty()
                    || intent
                        .external_reference
                        .as_ref()
                        .is_none_or(|s| s.trim().is_empty() || s.len() > 4096) =>
            {
                return Err(Error::InvalidInput(
                    "external execution requires a bounded reference and no managed command".into(),
                ));
            }
            _ => {}
        }
        let mut event = EventDraft::new(
            "execution.registered",
            "Registered execution before dispatch",
        );
        event.session_id = Some(session);
        self.runtime_transaction_with_event(project, expected, event, |tx, next, event| {
            let s = crate::session::session_at(tx, project, session)?;
            crate::workstream_runtime::require_current(tx, &s)?;
            let work = s.work_item_id.ok_or_else(|| {
                Error::InvalidInput("execution needs a work-bound session".into())
            })?;
            if s.status != "active" {
                return Err(Error::InvalidTransition(
                    "new execution needs an active session".into(),
                ));
            }
            crate::admission::require_with(
                tx,
                project,
                work,
                s.branch_id,
                Some(&intent.operation_key),
            )?;
            if key_at(tx, project, &intent.operation_key)?.is_some() {
                return Err(Error::SourceConflict(
                    "operation key already registered; read the existing execution".into(),
                ));
            }
            let record = Execution {
                id: Id::new(),
                project_id: project,
                work_item_id: work,
                session_id: session,
                branch_id: s.branch_id,
                revision: next,
                intent,
                state: ExecutionState::Registered,
                worker: None,
                registered_at: now_millis()?,
                started_at: None,
                finished_at: None,
                exit_code: None,
                signal: None,
                error: None,
                stdout: None,
                stderr: None,
                receipt: None,
            };
            event.payload = serde_json::json!({"execution":record});
            Ok(record)
        })
    }
    pub fn start_execution(
        &mut self,
        project: Id,
        expected: Revision,
        id: Id,
        worker: WorkerIdentity,
    ) -> Result<(Execution, Event)> {
        if worker.pid == 0 || worker.port == 0 || worker.child_pid.is_some() {
            return Err(Error::InvalidInput("invalid supervisor identity".into()));
        }
        self.change_execution(project, expected, id, "execution.starting", true, |e| {
            if e.intent.executor != ExecutorKind::ManagedLocal
                || e.state != ExecutionState::Registered
            {
                return Err(Error::InvalidTransition(
                    "execution was already dispatched or is external".into(),
                ));
            }
            e.state = ExecutionState::Starting;
            e.worker = Some(worker);
            e.started_at = Some(now_millis()?);
            e.stdout = Some(format!(".awr/executions/{id}/stdout.log"));
            e.stderr = Some(format!(".awr/executions/{id}/stderr.log"));
            e.receipt = Some(format!(".awr/executions/{id}/result.json"));
            Ok(())
        })
    }
    pub fn execution_running(
        &mut self,
        project: Id,
        expected: Revision,
        id: Id,
        nonce: Id,
        child: u32,
    ) -> Result<(Execution, Event)> {
        self.change_execution(project, expected, id, "execution.running", false, |e| {
            if e.state != ExecutionState::Starting
                || e.worker.as_ref().is_none_or(|w| w.nonce != nonce)
                || child == 0
            {
                return Err(Error::InvalidTransition(
                    "execution does not belong to this starting supervisor".into(),
                ));
            }
            e.worker.as_mut().unwrap().child_pid = Some(child);
            e.state = ExecutionState::Running;
            Ok(())
        })
    }
    pub fn finish_execution(
        &mut self,
        project: Id,
        expected: Revision,
        result: ExecutionResult,
    ) -> Result<(Execution, Event)> {
        ensure_public_data(&result)?;
        self.change_execution(
            project,
            expected,
            result.execution_id,
            "execution.finished",
            false,
            |e| {
                if !matches!(e.state, ExecutionState::Starting | ExecutionState::Running)
                    || e.worker.as_ref().is_none_or(|w| w.nonce != result.nonce)
                    || result.finished_at < e.started_at.unwrap_or(e.registered_at)
                    || result.error.as_ref().is_some_and(|s| s.len() > 8192)
                    || (result.success
                        && (result.exit_code != Some(0)
                            || result.signal.is_some()
                            || result.error.is_some()))
                {
                    return Err(Error::InvalidTransition(
                        "invalid execution completion receipt".into(),
                    ));
                }
                e.state = if result.success {
                    ExecutionState::Succeeded
                } else {
                    ExecutionState::Failed
                };
                e.finished_at = Some(result.finished_at);
                e.exit_code = result.exit_code;
                e.signal = result.signal;
                e.error = result.error;
                Ok(())
            },
        )
    }
    fn change_execution(
        &mut self,
        project: Id,
        expected: Revision,
        id: Id,
        kind: &str,
        admitted: bool,
        change: impl FnOnce(&mut Execution) -> Result<()>,
    ) -> Result<(Execution, Event)> {
        self.runtime_transaction_with_event(
            project,
            expected,
            EventDraft::new(kind, "Recorded managed execution observation"),
            |tx, next, event| {
                let mut e = at(tx, project, id)?;
                if admitted {
                    crate::admission::require_with(
                        tx,
                        project,
                        e.work_item_id,
                        e.branch_id,
                        Some(&e.intent.operation_key),
                    )?;
                }
                change(&mut e)?;
                e.revision = next;
                event.session_id = Some(e.session_id);
                event.work_item_id = Some(e.work_item_id);
                event.branch_id = e.branch_id;
                event.payload = serde_json::json!({"execution":e});
                Ok(e)
            },
        )
    }
}

fn report_at(conn: &Connection, project: Id, selector: &str, value: &str) -> Result<Option<Event>> {
    // selector is supplied only by the two typed readers above and remains a SQL parameter.
    let id:Option<String>=conn.query_row("SELECT id FROM events WHERE project_id=?1 AND event_type='execution.external_reported' AND json_extract(payload_json,?2)=?3 ORDER BY project_revision DESC LIMIT 1",params![project.to_string(),selector,value],|r|r.get(0)).optional().map_err(db_error)?;
    id.map(|id|conn.query_row("SELECT id,project_id,work_item_id,session_id,branch_id,event_type,importance,summary,payload_json,project_revision,created_at FROM events WHERE project_id=?1 AND id=?2",params![project.to_string(),id],crate::events::event_row).map_err(db_error)).transpose()
}
