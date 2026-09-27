//! Authenticated journals, coordination leases and scoped execution operations.
//! Ordinary commands validate work-scoped read sets (epoch, authority, ownership,
//! contract and action tokens). The project revision remains an ordered audit
//! cursor and is not treated as business CAS, so unrelated advances do not force
//! a semantic refresh. Writers still serialize on the project admission lock.
pub(crate) mod action_auth;
pub(crate) mod claims;
pub(crate) mod executions;
pub(crate) mod handoffs;
pub(crate) mod reviews;

use crate::workstream_auth::{
    CommandAuthPhase, ReaderAuthority, authenticate_writer, authorize_command,
};
use crate::workstream_read::{WorkstreamQuery, authenticated_read, work_binding};
use crate::{PgError, PgPool, PgResult};
use awr_core::Id;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio_postgres::Transaction;

pub(crate) const COMMANDS: &[&str] = &[
    "session.start",
    "session.checkpoint",
    "session.end",
    "claim.acquire",
    "claim.renew",
    "claim.release",
    "execution.prepare",
    "execution.cancel",
    "execution.start",
    "execution.report",
    "execution.attest",
    "execution.reconcile",
    "handoff.propose",
    "handoff.inspect",
    "handoff.accept",
    "handoff.reject",
    "handoff.cancel",
    "handoff.timeout",
    "evidence.submit",
    "review.open",
    "review.accept",
    "review.return",
    "review.decide",
    "work.rework",
    "work.complete",
    "delivery.submit_and_request_review",
    "delivery.register_pr",
    "delivery.observe_pr",
    "delivery.finalize",
];
const RECEIPT_PROTOCOL: &str = "awr-team-workstream-command-v1";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkstreamCommand {
    pub protocol_version: u32,
    pub request_id: String,
    pub op: String,
    pub workstream_id: Id,
    pub work_id: String,
    pub coordinator_epoch: String,
    pub expected_project_revision: String,
    pub expected_authority_version: String,
    pub expected_ownership_version: String,
    pub expected_contract_hash: String,
    pub args: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    conversation_id: String,
    client_info: Option<crate::feedback::ClientInfo>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    session_id: String,
    expected_session_version: String,
    context_hash: String,
    next_action: String,
    open_loops: Vec<String>,
    client_info: Option<crate::feedback::ClientInfo>,
    progress: Option<crate::feedback::Progress>,
    usage: Option<crate::feedback::Usage>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct End {
    session_id: String,
    expected_session_version: String,
}
enum Action {
    Start(Start),
    Checkpoint(Checkpoint),
    End(End),
    Claim(claims::Action),
    Execution(executions::Action),
    Handoff(handoffs::Action),
    Review(reviews::Action),
}

struct Applied {
    data: Value,
    preceding_events: Vec<(&'static str, Value)>,
}

fn invalid() -> PgError {
    PgError::invalid_command_fields()
}
fn identity(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control)
}
fn version(s: &str) -> PgResult<i64> {
    if s.is_empty() || (s.len() > 1 && s.starts_with('0')) || !s.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(invalid());
    }
    s.parse::<i64>().map_err(|_| invalid())
}
fn digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl WorkstreamCommand {
    pub const OPERATIONS: &'static [&'static str] = COMMANDS;

    fn action(&self) -> PgResult<Action> {
        if self.protocol_version != 1 || !COMMANDS.contains(&self.op.as_str()) {
            return Err(PgError::Unsupported("scoped command".into()));
        }
        if !identity(&self.request_id)
            || !identity(&self.work_id)
            || !identity(&self.coordinator_epoch)
            || !digest(&self.expected_contract_hash)
            || version(&self.expected_authority_version)? == 0
            || version(&self.expected_ownership_version)? == 0
        {
            return Err(invalid());
        }
        version(&self.expected_project_revision)?;
        match self.op.as_str() {
            "execution.prepare"
            | "execution.cancel"
            | "execution.start"
            | "execution.report"
            | "execution.attest"
            | "execution.reconcile" => Ok(Action::Execution(executions::Action::parse(
                &self.op,
                self.args.clone(),
            )?)),
            "claim.acquire" | "claim.renew" | "claim.release" => Ok(Action::Claim(
                claims::Action::parse(&self.op, self.args.clone())?,
            )),
            "handoff.propose" | "handoff.inspect" | "handoff.accept" | "handoff.reject"
            | "handoff.cancel" | "handoff.timeout" => Ok(Action::Handoff(handoffs::Action::parse(
                &self.op,
                self.args.clone(),
            )?)),
            "evidence.submit"
            | "review.open"
            | "review.accept"
            | "review.return"
            | "review.decide"
            | "work.rework"
            | "work.complete"
            | "delivery.submit_and_request_review"
            | "delivery.register_pr"
            | "delivery.observe_pr"
            | "delivery.finalize" => Ok(Action::Review(reviews::Action::parse(
                &self.op,
                self.args.clone(),
            )?)),
            "session.start" => {
                let a: Start = serde_json::from_value(self.args.clone()).map_err(|_| invalid())?;
                if !identity(&a.conversation_id) {
                    return Err(invalid());
                }
                if let Some(info) = &a.client_info {
                    info.validate()?;
                }
                Ok(Action::Start(a))
            }
            "session.checkpoint" => {
                let a: Checkpoint =
                    serde_json::from_value(self.args.clone()).map_err(|_| invalid())?;
                if !identity(&a.session_id)
                    || version(&a.expected_session_version)? == 0
                    || !digest(&a.context_hash)
                    || a.next_action.trim().is_empty()
                    || a.next_action.len() > 8192
                    || a.next_action.contains('\0')
                    || a.open_loops.len() > 32
                    || a.open_loops
                        .iter()
                        .any(|s| s.trim().is_empty() || s.len() > 4096 || s.contains('\0'))
                {
                    return Err(invalid());
                }
                if let Some(info) = &a.client_info {
                    info.validate()?;
                }
                if let Some(progress) = &a.progress {
                    progress.validate()?;
                }
                if let Some(usage) = &a.usage {
                    usage.validate()?;
                }
                Ok(Action::Checkpoint(a))
            }
            "session.end" => {
                let a: End = serde_json::from_value(self.args.clone()).map_err(|_| invalid())?;
                if !identity(&a.session_id) || version(&a.expected_session_version)? == 0 {
                    return Err(invalid());
                }
                Ok(Action::End(a))
            }
            _ => Err(invalid()),
        }
    }
}

pub struct WorkstreamCommandStore {
    pool: Arc<PgPool>,
}
impl WorkstreamCommandStore {
    pub fn new(url: impl Into<String>) -> Self {
        Self::from_pool(Arc::new(PgPool::new(url)))
    }
    pub fn from_config(config: tokio_postgres::Config) -> Self {
        Self::from_pool(Arc::new(PgPool::from_config(config)))
    }
    pub(crate) fn from_pool(pool: Arc<PgPool>) -> Self {
        Self { pool }
    }

    pub async fn execute(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        command: WorkstreamCommand,
    ) -> PgResult<Value> {
        let action = command.action()?;
        if serde_json::to_vec(&command).map_err(|_| invalid())?.len() > 65536 {
            return Err(invalid());
        }
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let auth = authenticate_writer(&tx, tenant, project, bearer).await?;
        let mut auth = auth;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        crate::delegation_auth::resolve_agent_delegation(
            &tx,
            &mut auth,
            project,
            Some(command.work_id.as_str()),
            // `action()` already validated this operation's argument shape.
            // Domain execution still verifies session owner, work and version.
            command.args.get("session_id").and_then(Value::as_str),
            crate::workstream_auth::command_business_action(&command.op),
            now_ms,
        )
        .await?;
        let (binding, ownership) =
            work_binding(&tx, tenant, project, &auth, &command.work_id).await?;
        let stream = binding.workstream_id;
        if stream != command.workstream_id {
            return Err(PgError::Forbidden);
        }
        // Shared domain-entry write boundary (HTTP/MCP/PG). Saving recovery notes
        // or ending a session may preserve work while its stream is paused; that
        // still requires a current explicit write grant at admission. Effect-phase
        // active-stream / attest / reconcile checks run after idempotent replay.
        // Project freeze/import/restore barriers remain stricter for all writes.
        authorize_command(
            &auth,
            stream,
            &command.work_id,
            &command.op,
            CommandAuthPhase::Admission,
        )?;
        if auth.epoch != command.coordinator_epoch {
            return Err(PgError::EpochChanged);
        }
        if ownership != version(&command.expected_ownership_version)? {
            return Err(PgError::PreconditionsChanged);
        }
        let request_hash = awr_team::request_hash(&json!({"protocol":RECEIPT_PROTOCOL,
            "tenant":tenant,"project":project,"actor":auth.actor_id,"client":auth.client_id,"command":command}))
            .map_err(|_| invalid())?;
        if let Some(row) = operation(&tx, tenant, project, &auth, &command.request_id).await? {
            if row.get::<_, String>(0) != request_hash || row.get::<_, String>(2) != "committed" {
                return Err(PgError::IdempotencyConflict);
            }
            let result: Value = row.get(1);
            check_receipt(
                &result,
                &auth,
                &command.work_id,
                &stream.to_string(),
                ownership,
            )?;
            tx.commit().await?;
            return Ok(json!({"replayed":true,"receipt":result,"execution_authorized":false}));
        }
        if auth.project_status != "active" {
            return Err(PgError::ProjectNotAvailable);
        }
        // expected_project_revision remains on the wire for legacy clients and is
        // validated as a decimal, but it is an audit cursor — not a business CAS.
        // Authority/ownership/contract/epoch (and action tokens) form the read set.
        let _audit_cursor = version(&command.expected_project_revision)?;
        if auth.catalog.get(stream)?.authority_version
            != version(&command.expected_authority_version)? as u64
        {
            return Err(PgError::PreconditionsChanged);
        }
        authorize_command(
            &auth,
            stream,
            &command.work_id,
            &command.op,
            CommandAuthPhase::Effect,
        )?;
        let stored = tx.query_one("SELECT contract_json,contract_hash FROM awr_team.work_contracts
            WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND scope_id='main' AND work_id=$4",
            &[&tenant,&project,&auth.snapshot,&command.work_id]).await?;
        let contract: awr_team::WorkContract =
            serde_json::from_value(stored.get(0)).map_err(|_| PgError::SourceDivergence)?;
        let contract_hash: String = stored.get(1);
        if contract.work_id.as_str() != command.work_id
            || contract.hash().map_err(|_| PgError::SourceDivergence)? != contract_hash
        {
            return Err(PgError::SourceDivergence);
        }
        if contract_hash != command.expected_contract_hash {
            return Err(PgError::PreconditionsChanged);
        }
        let applied = match action {
            Action::Execution(a) => {
                executions::apply(
                    &tx, tenant, project, &auth, &command, ownership, &contract, a,
                )
                .await?
            }
            Action::Claim(a) => {
                claims::apply(&tx, tenant, project, &auth, &command, ownership, a).await?
            }
            Action::Handoff(a) => {
                handoffs::apply(&tx, tenant, project, &auth, &command, ownership, a).await?
            }
            Action::Review(a) => {
                reviews::apply(
                    &tx, tenant, project, &auth, &command, ownership, &contract, a,
                )
                .await?
            }
            a => Applied {
                data: apply(&tx, tenant, project, bearer, &auth, &command, ownership, a).await?,
                preceding_events: Vec::new(),
            },
        };
        let mut data = applied.data;
        if command.op.starts_with("claim.") {
            data["lease_state_basis"] = json!("at_commit");
        }
        if command.op.starts_with("execution.") {
            data["execution_state_basis"] = json!("at_commit");
        }
        if command.op.starts_with("handoff.") {
            data["handoff_state_basis"] = json!("at_commit");
        }
        if command.op.starts_with("evidence.")
            || command.op.starts_with("review.")
            || matches!(command.op.as_str(), "work.rework" | "work.complete")
        {
            data["review_state_basis"] = json!("at_commit");
        }
        // Project writers still take an exclusive admission lock (SQLite single-writer
        // compatible serialization). The audit cursor advances here without treating
        // the caller's expected_project_revision as business CAS.
        let next = auth
            .revision
            .checked_add(1)
            .ok_or(PgError::PreconditionsChanged)?;
        tx.execute(
            "UPDATE awr_team.projects SET project_revision=$3 WHERE tenant_id=$1 AND id=$2",
            &[&tenant, &project, &next],
        )
        .await?;
        let receipt = json!({"protocol":RECEIPT_PROTOCOL,"request_id":command.request_id,"op":command.op,
            "request_hash":request_hash,
            "work_id":command.work_id,"workstream_id":stream,"scope_id":"main","ownership_version":ownership.to_string(),
            "authority_version":auth.catalog.get(stream)?.authority_version.to_string(),
            "coordinator_epoch":auth.epoch,"source_snapshot_id":auth.snapshot,"contract_hash":contract_hash,
            "committed_project_revision":next.to_string(),"data":data,"execution_authorized":false});
        // State, attribution, monotonically ordered event and replay receipt are
        // one transaction. No network work or source scan occurs under this lock.
        for (index, (kind, payload)) in applied
            .preceding_events
            .iter()
            .map(|(kind, payload)| (*kind, payload))
            .chain(std::iter::once((command.op.as_str(), &data)))
            .enumerate()
        {
            tx.execute("INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,work_id,payload_json,workstream_id)
                VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
                &[&tenant,&project,&crate::tx::new_id(),&next,&(index as i32),&kind,&auth.actor_id,&command.work_id,&payload,&stream.to_string()]).await?;
        }
        tx.execute("INSERT INTO awr_team.operations(tenant_id,project_id,id,actor_id,client_id,request_id,op,request_hash,state,committed_project_revision,result_json)
            VALUES($1,$2,$3,$4,$5,$6,$7,$8,'committed',$9,$10)",
            &[&tenant,&project,&crate::tx::new_id(),&auth.actor_id,&auth.client_id,&command.request_id,&command.op,&request_hash,&next,&receipt]).await?;
        // Delivery/review ops audit belongs in THIS transaction (authenticated MCP
        // path), not only ReviewStore entrypoints — TMCP-040 CR on PR #134.
        record_command_delivery_ops_audit(&tx, tenant, project, &auth, &command, stream, &data)
            .await?;
        tx.commit().await?;
        // Only the original committed start response permits one caller-managed
        // execution. Stored/replayed receipts are historical, never a new grant.
        Ok(
            json!({"replayed":false,"receipt":receipt,"execution_authorized":command.op == "execution.start"}),
        )
    }
}

/// Bind delivery/review ops-audit to the authenticated command transaction.
/// Covers MCP `review.decide` / `delivery.register_pr` / `delivery.finalize`
/// (and siblings) so audit.history by request_id sees the real path.
async fn record_command_delivery_ops_audit(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    stream: Id,
    data: &Value,
) -> PgResult<()> {
    let (audit_action, target_kind, target_id_key) = match command.op.as_str() {
        "review.decide" | "review.accept" | "review.return" => {
            ("review.decide", "review", "round_id")
        }
        "delivery.register_pr" => ("delivery.register_pr", "delivery", "delivery_id"),
        "delivery.observe_pr" => ("delivery.observe_pr", "delivery", "delivery_id"),
        "work.complete" | "delivery.finalize" => ("delivery.finalize", "completion", "receipt_id"),
        _ => return Ok(()),
    };
    #[cfg(feature = "pg-tests")]
    if command.request_id == "inject-ops-audit-abort" {
        return Err(PgError::Protocol("injected ops audit abort".into()));
    }
    let mut summary = data.clone();
    if let Some(obj) = summary.as_object_mut() {
        obj.insert("op".into(), json!(command.op.clone()));
        obj.insert("request_id".into(), json!(command.request_id.clone()));
        obj.insert(
            "expected_contract_hash".into(),
            json!(command.expected_contract_hash.clone()),
        );
        obj.insert(
            "coordinator_epoch".into(),
            json!(command.coordinator_epoch.clone()),
        );
        obj.insert(
            "expected_ownership_version".into(),
            json!(command.expected_ownership_version.clone()),
        );
        obj.insert(
            "expected_authority_version".into(),
            json!(command.expected_authority_version.clone()),
        );
    }
    let mut audit = crate::ops_audit::write_from_auth(
        auth,
        crate::ops_audit::OpsCategory::Delivery,
        audit_action,
        target_kind,
    );
    audit.target_id = data
        .get(target_id_key)
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    audit.work_id = Some(command.work_id.clone());
    audit.request_id = Some(command.request_id.clone());
    audit.authority_version = auth
        .catalog
        .get(stream)
        .ok()
        .map(|entry| entry.authority_version as i64);
    audit.person_id = data
        .get("reviewer_person_id")
        .or_else(|| data.get("approved_by_person_id"))
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    audit.digest = Some(crate::ops_audit::digest_of(&summary));
    audit.summary = summary;
    crate::ops_audit::record_in_tx(tx, tenant, project, &audit).await?;
    Ok(())
}

async fn apply(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    bearer: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    ownership: i64,
    action: Action,
) -> PgResult<Value> {
    match action {
        Action::Claim(_) | Action::Execution(_) | Action::Handoff(_) | Action::Review(_) => {
            Err(invalid())
        } // Same outer transaction.
        Action::Start(a) => {
            let active: bool = tx.query_one("SELECT EXISTS(SELECT 1 FROM awr_team.sessions
                WHERE tenant_id=$1 AND project_id=$2 AND actor_id=$3 AND client_id=$4 AND conversation_id=$5 AND work_id=$6 AND state='active')",
                &[&tenant,&project,&auth.actor_id,&auth.client_id,&a.conversation_id,&command.work_id]).await?.get(0);
            if active {
                return Err(PgError::PreconditionsChanged);
            }
            let session = crate::tx::new_id();
            tx.execute("INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state,workstream_id,ownership_version)
                VALUES($1,$2,$3,'main',$4,$5,$6,$7,'active',$8,$9)",
                &[&tenant,&project,&session,&command.work_id,&auth.actor_id,&auth.client_id,&a.conversation_id,&command.workstream_id.to_string(),&ownership]).await?;
            if let Some(info) = &a.client_info {
                tx.execute("UPDATE awr_team.sessions SET client_info_json=$4,client_info_at=clock_timestamp()
                    WHERE tenant_id=$1 AND project_id=$2 AND id=$3", &[&tenant,&project,&session,&json!(info)]).await?;
            }
            Ok(json!({"session_id":session,"session_version":"1","state":"active"}))
        }
        Action::Checkpoint(a) => {
            let current = session(
                tx,
                tenant,
                project,
                auth,
                command,
                &a.session_id,
                &a.expected_session_version,
                ownership,
            )
            .await?;
            let query: WorkstreamQuery = serde_json::from_value(
                json!({"protocol_version":1,"op":"work.prepare","work_id":command.work_id,
                "session_id":a.session_id,"max_context_bytes":262144}),
            )
            .map_err(|_| invalid())?;
            let context = authenticated_read(tx, tenant, project, bearer, &query).await?;
            if context["data"]["context_hash"] != a.context_hash {
                return Err(PgError::PreconditionsChanged);
            }
            crate::feedback::validate_support(
                tx,
                tenant,
                project,
                &a.session_id,
                a.client_info.as_ref(),
                a.progress.is_some(),
                a.usage.is_some(),
            )
            .await?;
            if let Some(usage) = &a.usage {
                usage
                    .check_order(tx, tenant, project, &a.session_id)
                    .await?;
            }
            let checkpoint = crate::tx::new_id();
            tx.execute("INSERT INTO awr_team.checkpoints(tenant_id,project_id,id,session_id,context_hash,contract_hash,observed_revision,next_action,open_loops_json,progress_json,usage_json)
                VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
                &[&tenant,&project,&checkpoint,&a.session_id,&a.context_hash,&command.expected_contract_hash,&auth.revision,&a.next_action,&json!(a.open_loops),&a.progress.as_ref().map(|v|json!(v)),&a.usage.as_ref().map(|v|json!(v))]).await?;
            if let Some(info) = &a.client_info {
                tx.execute("UPDATE awr_team.sessions SET client_info_json=$4,client_info_at=clock_timestamp()
                    WHERE tenant_id=$1 AND project_id=$2 AND id=$3", &[&tenant,&project,&a.session_id,&json!(info)]).await?;
            }
            tx.execute("UPDATE awr_team.sessions SET latest_checkpoint_id=$4,session_version=session_version+1
                WHERE tenant_id=$1 AND project_id=$2 AND id=$3",&[&tenant,&project,&a.session_id,&checkpoint]).await?;
            Ok(
                json!({"session_id":a.session_id,"session_version":(current+1).to_string(),"checkpoint_id":checkpoint,
                "context_hash":a.context_hash,"context_hash_basis":"recomputed_scoped_context","state":"active"}),
            )
        }
        Action::End(a) => {
            let current = session(
                tx,
                tenant,
                project,
                auth,
                command,
                &a.session_id,
                &a.expected_session_version,
                ownership,
            )
            .await?;
            let blocked: bool = tx.query_one("SELECT
                EXISTS(SELECT 1 FROM awr_team.claims WHERE tenant_id=$1 AND project_id=$2 AND session_id=$3 AND state='active') OR
                EXISTS(SELECT 1 FROM awr_team.executions WHERE tenant_id=$1 AND project_id=$2 AND work_id=$4 AND state NOT IN ('succeeded','failed','cancelled')) OR
                EXISTS(SELECT 1 FROM awr_team.wait_items WHERE tenant_id=$1 AND project_id=$2 AND session_id=$3 AND state='open')",
                &[&tenant,&project,&a.session_id,&command.work_id]).await?.get(0);
            if blocked {
                return Err(PgError::RecoveryBlocked);
            }
            tx.execute(
                "UPDATE awr_team.sessions SET state='ended',session_version=session_version+1
                WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant, &project, &a.session_id],
            )
            .await?;
            Ok(
                json!({"session_id":a.session_id,"session_version":(current+1).to_string(),"state":"ended"}),
            )
        }
    }
}

async fn session(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    id: &str,
    expected: &str,
    ownership: i64,
) -> PgResult<i64> {
    let r = tx.query_opt("SELECT work_id,workstream_id,ownership_version,actor_id,client_id,session_version,state
        FROM awr_team.sessions WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND scope_id='main' FOR UPDATE",
        &[&tenant,&project,&id]).await?.ok_or(PgError::Forbidden)?;
    if r.get::<_, String>(0) != command.work_id
        || r.get::<_, Option<String>>(1) != Some(command.workstream_id.to_string())
        || r.get::<_, Option<i64>>(2) != Some(ownership)
        || r.get::<_, String>(3) != auth.actor_id
        || r.get::<_, String>(4) != auth.client_id
    {
        return Err(PgError::Forbidden);
    }
    let current: i64 = r.get(5);
    if current != version(expected)? || current == i64::MAX || r.get::<_, String>(6) != "active" {
        return Err(PgError::PreconditionsChanged);
    }
    Ok(current)
}

async fn operation(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    request: &str,
) -> PgResult<Option<tokio_postgres::Row>> {
    Ok(tx.query_opt("SELECT request_hash,result_json,state FROM awr_team.operations WHERE tenant_id=$1 AND project_id=$2
        AND actor_id=$3 AND client_id=$4 AND request_id=$5",
        &[&tenant,&project,&auth.actor_id,&auth.client_id,&request]).await?)
}
fn check_receipt(
    result: &Value,
    auth: &ReaderAuthority,
    work: &str,
    stream: &str,
    ownership: i64,
) -> PgResult<()> {
    if result["protocol"] != RECEIPT_PROTOCOL
        || result["work_id"] != work
        || result["workstream_id"] != stream
        || result["ownership_version"] != ownership.to_string()
        || result["coordinator_epoch"] != auth.epoch
    {
        return Err(PgError::Forbidden);
    }
    Ok(())
}
pub(crate) async fn inspect(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    request: &str,
    work: &str,
    stream: &str,
    ownership: i64,
) -> PgResult<Value> {
    if let Some(row) = operation(tx, tenant, project, auth, request).await? {
        if row.get::<_, String>(2) != "committed" {
            return Err(PgError::Forbidden);
        }
        let receipt: Value = row.get(1);
        check_receipt(&receipt, auth, work, stream, ownership)?;
        Ok(json!({"state":"committed","receipt":receipt}))
    } else {
        // Absence is not proof that a timed-out concurrent command cannot commit.
        Ok(json!({"state":"unknown","request_id":request,"retry":"same_request_id_and_payload"}))
    }
}
