//! Authenticated, scope-limited Team read operations. The transport binds the
//! tenant/project from operator configuration; request bodies contain selectors.
use crate::workstream_auth::{ReaderAuthority, authenticate, authorize_query};
use crate::{PgError, PgPool, PgResult};
use awr_core::{
    Id, WorkstreamAction, WorkstreamSelection, WorkstreamSessionBinding, WorkstreamWorkBinding,
    resolve_workstream,
};
use awr_team::WorkContract;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_postgres::{IsolationLevel, Transaction};

mod activity;
mod guidance;
mod navigation;
mod observation;

const QUERIES: &[&str] = &[
    "capabilities",
    "workstreams.list",
    "work.list",
    "work.search",
    "work.next",
    "work.prepare",
    "work.observe",
    "events.list",
    "session.inspect",
    "work.recovery",
    "command.inspect",
    "claim.inspect",
    "execution.inspect",
    "handoff.inspect",
    "evidence.inspect",
    "review.inspect",
    "completion.inspect",
    "delivery.inspect",
    "source.content",
    "artifact.content",
    "planning.outcome",
    "audit.history",
    "audit.export",
    "audit.count",
    "audit.requests",
    "audit.development",
];

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkstreamQuery {
    pub protocol_version: u32,
    pub op: String,
    pub workstream_id: Option<Id>,
    pub work_id: Option<String>,
    pub session_id: Option<String>,
    pub search: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u16>,
    pub max_context_bytes: Option<usize>,
    pub request_id: Option<String>,
    pub claim_id: Option<String>,
    pub execution_id: Option<String>,
    pub handoff_id: Option<String>,
    pub evidence_id: Option<String>,
    pub review_round_id: Option<String>,
    /// Controlled source path relative to the active snapshot (TMCP-023).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    /// Controlled artifact id (TMCP-023).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<String>,
    /// Optional expected digest for content reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_sha256: Option<String>,
    /// Ops-audit filters (TMCP-040).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_actor_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_denies: Option<bool>,
}

impl WorkstreamQuery {
    pub const OPERATIONS: &'static [&'static str] = QUERIES;

    pub fn validate(&self) -> PgResult<()> {
        if self.protocol_version != 1 {
            return Err(PgError::Unsupported(
                "workstream query protocol version".into(),
            ));
        }
        if !QUERIES.contains(&self.op.as_str()) {
            return Err(PgError::Unsupported("workstream query operation".into()));
        }
        for id in [
            &self.work_id,
            &self.session_id,
            &self.request_id,
            &self.claim_id,
            &self.execution_id,
            &self.evidence_id,
            &self.review_round_id,
            &self.artifact_id,
            &self.change_id,
            &self.member_actor_id,
            &self.category,
        ]
        .into_iter()
        .flatten()
        {
            if id.is_empty() || id.len() > 128 || id.chars().any(char::is_control) {
                return Err(PgError::Protocol("invalid selector".into()));
            }
        }
        if self.limit.is_some_and(|n| n == 0 || n > 100)
            || self.cursor.as_ref().is_some_and(|s| s.len() > 4096)
            || self.max_context_bytes.is_some_and(|n| n == 0 || n > 262144)
        {
            return Err(PgError::Protocol("query bounds exceeded".into()));
        }
        if self.op == "work.next"
            && (self.workstream_id.is_some() || self.work_id.is_some() || self.session_id.is_some())
        {
            return Err(PgError::work_next_selectors());
        }
        let paged = matches!(
            self.op.as_str(),
            "workstreams.list" | "work.list" | "work.search" | "events.list" | "work.next"
        );
        let audit = matches!(
            self.op.as_str(),
            "audit.history"
                | "audit.export"
                | "audit.count"
                | "audit.requests"
                | "audit.development"
        );
        if !paged && !audit && (self.cursor.is_some() || self.limit.is_some())
            || self.search.is_some() != (self.op == "work.search")
            || self
                .search
                .as_ref()
                .is_some_and(|s| s.is_empty() || s.len() > 512 || s.chars().any(char::is_control))
            || self.max_context_bytes.is_some()
                && !matches!(
                    self.op.as_str(),
                    "work.prepare" | "source.content" | "artifact.content"
                )
            || (!audit
                && self.request_id.is_some()
                    != matches!(self.op.as_str(), "command.inspect" | "planning.outcome"))
            || (self.change_id.is_some()
                || self.member_actor_id.is_some()
                || self.category.is_some()
                || self.include_denies.is_some())
                && !audit
            || self.claim_id.is_some() != (self.op == "claim.inspect")
            || self.execution_id.is_some() != (self.op == "execution.inspect")
            || self.handoff_id.is_some() != (self.op == "handoff.inspect")
            || self.evidence_id.is_some() != (self.op == "evidence.inspect")
            || self.review_round_id.is_some() != (self.op == "review.inspect")
            || self.source_path.is_some() != (self.op == "source.content")
            || self.artifact_id.is_some() != (self.op == "artifact.content")
            || self.expected_sha256.as_ref().is_some_and(|s| {
                s.len() != 64
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
            || self.source_path.as_ref().is_some_and(|path| {
                path.is_empty()
                    || path.len() > 512
                    || path.starts_with('/')
                    || path.contains("..")
                    || path.contains('\\')
                    || path.contains(':')
                    || path.contains("://")
                    || path.chars().any(char::is_control)
            })
            || matches!(self.op.as_str(), "capabilities" | "workstreams.list")
                && (self.work_id.is_some()
                    || self.session_id.is_some()
                    || self.workstream_id.is_some())
            || matches!(self.op.as_str(), "work.list" | "work.search")
                && (self.work_id.is_some() || self.session_id.is_some())
            || matches!(self.op.as_str(), "audit.requests" | "audit.development")
                && (self.session_id.is_some()
                    || self.workstream_id.is_some()
                    || self.change_id.is_some()
                    || self.category.is_some()
                    || self.request_id.is_some()
                    || self.include_denies.is_some())
            || self.op == "session.inspect" && self.session_id.is_none()
            || self.op == "planning.outcome"
                && (self.work_id.is_some() || self.session_id.is_some())
            || matches!(
                self.op.as_str(),
                "work.prepare"
                    | "work.observe"
                    | "work.recovery"
                    | "command.inspect"
                    | "claim.inspect"
                    | "execution.inspect"
                    | "handoff.inspect"
                    | "evidence.inspect"
                    | "review.inspect"
                    | "completion.inspect"
                    | "delivery.inspect"
                    | "artifact.content"
            ) && self.work_id.is_none()
                && self.session_id.is_none()
        {
            return Err(PgError::Protocol(
                "query fields do not match operation".into(),
            ));
        }
        Ok(())
    }
}

pub struct WorkstreamReadStore {
    pool: std::sync::Arc<PgPool>,
}

#[cfg(test)]
mod input_guidance_tests {
    use super::*;

    #[test]
    fn work_next_rejects_selectors_with_specific_guidance() {
        for (field, value) in [
            ("workstream_id", "01ARZ3NDEKTSV4RRFFQ69G5FAV"),
            ("work_id", "private-work-sentinel"),
            ("session_id", "private-session-sentinel"),
        ] {
            let mut input = json!({"protocol_version":1,"op":"work.next"});
            input[field] = json!(value);
            let query: WorkstreamQuery = serde_json::from_value(input).unwrap();
            let error = query.validate().unwrap_err();
            assert!(error.is_work_next_selectors());
            assert!(!error.to_string().contains(value));
        }
    }

    #[test]
    fn work_next_guidance_preserves_pagination_and_other_validation() {
        for input in [
            json!({"protocol_version":1,"op":"work.next"}),
            json!({"protocol_version":1,"op":"work.next","limit":1,"cursor":"opaque"}),
        ] {
            let query: WorkstreamQuery = serde_json::from_value(input).unwrap();
            assert!(query.validate().is_ok());
        }
        for input in [
            json!({"protocol_version":1,"op":"work.next","limit":0}),
            json!({"protocol_version":1,"op":"work.next","search":"unexpected"}),
            json!({"protocol_version":1,"op":"capabilities","work_id":"a"}),
            json!({"protocol_version":1,"op":"unsupported","work_id":"a"}),
        ] {
            let query: WorkstreamQuery = serde_json::from_value(input).unwrap();
            assert!(!query.validate().unwrap_err().is_work_next_selectors());
        }
    }
}

impl WorkstreamReadStore {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            pool: std::sync::Arc::new(PgPool::new(url)),
        }
    }
    pub fn from_config(config: tokio_postgres::Config) -> Self {
        Self {
            pool: std::sync::Arc::new(PgPool::from_config(config)),
        }
    }

    /// Use the same pool and authority boundary for durable session commands.
    pub fn commands(&self) -> crate::WorkstreamCommandStore {
        crate::WorkstreamCommandStore::from_pool(self.pool.clone())
    }

    /// Project-admin member/role/credential management (TMCP-012).
    pub fn project_access(&self) -> crate::ProjectAccessStore {
        crate::ProjectAccessStore::from_pool(self.pool.clone())
    }

    /// Authoritative source / planning domain entry (TMCP-021/022/023).
    pub fn source(&self) -> crate::SourceStore {
        crate::SourceStore::from_pool(self.pool.clone())
    }

    pub async fn check_schema(&self) -> PgResult<()> {
        let client = self.pool.get().await?;
        crate::check_schema(&client).await
    }

    pub fn ops_audit(&self) -> crate::OpsAuditStore {
        crate::OpsAuditStore::from_pool(self.pool.clone())
    }

    pub async fn query(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        request: WorkstreamQuery,
    ) -> PgResult<Value> {
        if matches!(
            request.op.as_str(),
            "audit.history" | "audit.export" | "audit.count"
        ) {
            request.validate()?;
            let filter = crate::OpsHistoryFilter {
                work_id: request.work_id.clone(),
                change_id: request.change_id.clone(),
                member_actor_id: request.member_actor_id.clone(),
                request_id: request.request_id.clone(),
                category: request.category.clone(),
                include_denies: request.include_denies.unwrap_or(false),
                limit: request.limit.map(|n| n as i64),
            };
            let store = self.ops_audit();
            return match request.op.as_str() {
                "audit.history" => store.history(tenant, project, bearer, &filter).await,
                "audit.export" => store.export(tenant, project, bearer, &filter).await,
                "audit.count" => store.count(tenant, project, bearer, &filter).await,
                _ => unreachable!(),
            };
        }
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let result = authenticated_read(&tx, tenant, project, bearer, &request).await?;
        if serde_json::to_vec(&result)
            .map_err(|_| PgError::SourceDivergence)?
            .len()
            > 1_048_576
        {
            return Err(PgError::ResponseTooLarge);
        }
        // No mutation or reusable authority object escapes this transaction.
        tx.commit().await?;
        Ok(result)
    }
}

/// Native reads and checkpoint verification use the identical read authority.
/// A command's action grant is enforced separately and must not replace the
/// WorkRead grant or broaden the source set used to compute consumed context.
pub(crate) async fn authenticated_read(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    bearer: &str,
    request: &WorkstreamQuery,
) -> PgResult<Value> {
    let mut auth = authenticate(tx, tenant, project, bearer).await?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    crate::delegation_auth::resolve_agent_read_delegation(tx, &mut auth, project, request, now_ms)
        .await?;
    read(tx, tenant, project, &auth, request).await
}

#[derive(Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Cursor {
    binding: String,
    key: String,
    revision: String,
    index: i32,
}

fn hash(value: &Value) -> PgResult<String> {
    awr_team::request_hash(value).map_err(|_| PgError::SourceDivergence)
}

fn cursor(request: &WorkstreamQuery, binding: &str) -> PgResult<Cursor> {
    let c = match &request.cursor {
        Some(raw) => serde_json::from_str::<Cursor>(raw).map_err(|_| PgError::CursorExpired)?,
        None => Cursor {
            binding: binding.into(),
            revision: "0".into(),
            index: -1,
            ..Default::default()
        },
    };
    if c.binding != binding || c.revision.parse::<i64>().is_err() {
        return Err(PgError::CursorExpired);
    }
    Ok(c)
}

fn next_cursor(binding: &str, key: &str, revision: i64, index: i32) -> Value {
    Value::String(
        serde_json::to_string(&Cursor {
            binding: binding.into(),
            key: key.into(),
            revision: revision.to_string(),
            index,
        })
        .expect("cursor serialization"),
    )
}

pub(crate) async fn work_binding(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    work: &str,
) -> PgResult<(WorkstreamWorkBinding, i64)> {
    let row = tx
        .query_opt(
            "SELECT workstream_id,ownership_version FROM awr_team.workstream_snapshot_ownership
        WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND work_id=$4",
            &[&tenant, &project, &auth.snapshot, &work],
        )
        .await?
        .ok_or(PgError::Forbidden)?;
    let id: Id = row
        .get::<_, String>(0)
        .parse()
        .map_err(|_| PgError::SourceDivergence)?;
    auth.access
        .authorize(&auth.catalog, id, WorkstreamAction::Read)
        .map_err(|_| PgError::Forbidden)?;
    Ok((
        WorkstreamWorkBinding {
            project_id: project.into(),
            work_item_id: work.into(),
            workstream_id: id,
        },
        row.get(1),
    ))
}

pub(crate) async fn read(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    q: &WorkstreamQuery,
) -> PgResult<Value> {
    let visible: Vec<_> = auth
        .catalog
        .workstreams
        .iter()
        .filter(|s| {
            auth.access
                .authorize(&auth.catalog, s.id, WorkstreamAction::Read)
                .is_ok()
        })
        .collect();
    if visible.is_empty() {
        return Err(PgError::Forbidden);
    }
    q.validate()?;
    // TMCP-011: every query shares the work.read decision; invisible streams stay filtered above.
    authorize_query(auth, None, q.work_id.as_deref(), &q.op)?;
    if q.op == "work.next" {
        return navigation::next(tx, tenant, project, auth, q).await;
    }
    if matches!(q.op.as_str(), "audit.requests" | "audit.development") {
        return activity::read(tx, tenant, project, auth, q).await;
    }
    if q.op == "capabilities" {
        let mut caps = json!({
            "protocol":"awr-team-workstream","protocol_version":1,"queries":QUERIES,
            "commands":crate::workstream_command::COMMANDS,
            "authentication":"bearer_per_request",
            "command_preconditions":"project_revision_v1","command_status_query":"command.inspect",
            "claim_semantics":"coordination_only","lease_ttl_seconds":{"min":1,"max":3600},
            "execution_intents":true,"execution_dispatch":false,"execution_start":true,"execution_reports":true,
            "execution_modes":["caller_managed","reference_write_v1"],"execution_report_authority":"caller_asserted",
            "reference_runner":{"transport":"operator_local_cli_or_library","effects":"bounded_file_writes",
                "requires":"system_actor_with_explicit_attestation_grant","fencing_class":"uncontrolled","max_plan_bytes":1048576},
            "execution_reconciliation":true,"trusted_execution_results":true,
            "previous_epoch_reconciliation":true,"unattributed_history_adoption":false,"operator_history_migration":"schema_owner_cli_sessions_inactive_claims_events_v1",
            "execution_trust_authority":"explicit_operator_grant_and_actor_kind",
            "dependency_exports":false,"execution_admission":true,
            "execution_admission_scope":"same_client_current_lease_lexical_project_resources",
            "artifact_content":true,
            "session_feedback":{
                "version":1,"start_optional_fields":["client_info"],
                "checkpoint_optional_fields":["client_info","progress","usage"],
                "read":"work.observe","visibility":"work_read_business_summary",
                "provenance":"caller_declared","nonterminal":true,
                "usage_scope":"host_session","usage_aggregation":"none",
                "billing_collected":false,"stale_after_ms":crate::feedback::STALE_AFTER_MS
            },
            "source_content":true,
            "controlled_content_reads":true,
            "planning": crate::SourceStore::planning_capabilities(),
            "planning_writeback": crate::SourceStore::planning_writeback_capabilities(),
            "planning_mcp": {
                "tools": [
                    "awr_team_planning_suggest",
                    "awr_team_planning_draft",
                    "awr_team_planning_preview",
                    "awr_team_planning_approve",
                    "awr_team_planning_publish",
                    "awr_team_planning_outcome"
                ],
                "sql_tools": false,
                "arbitrary_file_edit": false,
                "direct_done": false,
                "idempotent_request_id": true,
                "outcome_query": "planning.outcome"
            },
            "ops_audit": {
                "protocol": "awr-ops-audit-v1",
                "queries": ["audit.history", "audit.export", "audit.count"],
                "project_wide_action": "audit.read_project",
                "deny_capacity_per_project": crate::DENY_CAPACITY_PER_PROJECT,
                "chat_text_collected": false,
                "tool_io_collected": false,
                "token_billing_collected": false,
                "non_repudiation": "not_claimed_against_db_owner"
            }
        });
        caps["agent_review"] = json!({
            "command":"review.decide", "policy":crate::review::AGENT_REVIEW_POLICY,
            "requires":["agent_actor","agent_review_membership_grant","live_review_delegation","distinct_author_actor_and_client"],
            "approval_basis":"agent_review", "human_approval":false,
            "team_independent_acceptance":false, "completion_supported":true,
            "execution_basis":"caller_asserted_reconciled", "dependency_adoption":"explicit_v2_same_stream_per_predecessor",
            "dependency_acceptance_mode":"agent_reviewed_caller_asserted_reconciled",
            "unmapped_agent_dependencies":"blocked",
            "cross_workstream_adoption":"human_independent_only", "existing_same_stream_policies":"unchanged",
            "legacy_human_review_aliases":["review.accept","review.return"]
        });
        caps["identity"] = navigation::identity(auth);
        caps["project_entry"] = json!("work.next");
        // WS-014: explicit scope=main / old-client / local-file boundaries.
        if let Some(obj) = caps.as_object_mut() {
            obj.extend(
                crate::workstream_auth::workstream_boundary_capabilities()
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
            );
        }
        return Ok(caps);
    }
    if q.op == "workstreams.list" {
        let binding = hash(
            &json!({"auth":auth.binding,"snapshot":auth.snapshot,"op":q.op,
            "grants":visible.iter().map(|s|(s.id,s.authority_version,auth.grant_versions[&s.id])).collect::<Vec<_>>()}),
        )?;
        let c = cursor(q, &binding)?;
        let limit = usize::from(q.limit.unwrap_or(50));
        let mut sorted = visible.clone();
        sorted.sort_by_key(|s| s.id);
        let rows: Vec<_> = sorted
            .into_iter()
            .filter(|s| s.id.to_string() > c.key)
            .take(limit + 1)
            .collect();
        let more = rows.len() > limit;
        let items: Vec<_> = rows.into_iter().take(limit).collect();
        let next = if more {
            next_cursor(&binding, &items.last().expect("page").id.to_string(), 0, -1)
        } else {
            Value::Null
        };
        return Ok(json!({"items":items,"total":visible.len(),"next_cursor":next}));
    }
    if q.op == "planning.outcome" {
        let request_id = q.request_id.as_deref().ok_or(PgError::Forbidden)?;
        let row = tx
            .query_opt(
                "SELECT op, request_hash, result_json, created_at::text
                 FROM awr_team.planning_command_receipts
                 WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
                &[&tenant, &project, &request_id],
            )
            .await?;
        let data = match row {
            Some(r) => json!({
                "protocol":"awr-team-planning-command-v1",
                "request_id":request_id,
                "op":r.get::<_,String>(0),
                "request_hash":r.get::<_,String>(1),
                "result":r.get::<_,Value>(2),
                "created_at":r.get::<_,String>(3),
                "already_recorded":true,
                "next_step":"reuse this receipt; do not resubmit with a new request_id"
            }),
            None => json!({
                "protocol":"awr-team-planning-command-v1",
                "request_id":request_id,
                "already_recorded":false,
                "result":null,
                "next_step":"absent receipt is unknown — wait/retry inspect before submitting a new request_id"
            }),
        };
        return Ok(json!({
            "protocol_version":1,
            "scope_id":"main",
            "selection_basis":"project",
            "coordinator_epoch":auth.epoch,
            "project_status":auth.project_status,
            "source_snapshot_id":auth.snapshot,
            "project_revision":auth.revision.to_string(),
            "data":data
        }));
    }
    if q.op == "source.content" {
        let path = q.source_path.as_deref().ok_or(PgError::Forbidden)?;
        let data = read_controlled_source_content(
            tx,
            tenant,
            project,
            auth,
            path,
            q.expected_sha256.as_deref(),
            q.max_context_bytes,
        )
        .await?;
        return Ok(json!({
            "protocol_version":1,
            "scope_id":"main",
            "selection_basis":"project",
            "coordinator_epoch":auth.epoch,
            "project_status":auth.project_status,
            "source_snapshot_id":auth.snapshot,
            "project_revision":auth.revision.to_string(),
            "data":data
        }));
    }
    let mut selection = WorkstreamSelection {
        explicit: q.workstream_id,
        ..Default::default()
    };
    if let Some(work) = &q.work_id {
        selection.work = Some(work_binding(tx, tenant, project, auth, work).await?.0);
    }
    if let Some(session) = &q.session_id {
        let row = tx
            .query_opt(
                "SELECT work_id,workstream_id,ownership_version FROM awr_team.sessions
            WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND scope_id='main'",
                &[&tenant, &project, &session],
            )
            .await?
            .ok_or(PgError::Forbidden)?;
        let work: String = row.get(0);
        let (binding, generation) = work_binding(tx, tenant, project, auth, &work).await?;
        if row.get::<_, Option<String>>(1) != Some(binding.workstream_id.to_string())
            || row.get::<_, Option<i64>>(2) != Some(generation)
        {
            return Err(PgError::Forbidden);
        }
        selection.session = Some(WorkstreamSessionBinding {
            session_id: session.clone(),
            work: binding,
        });
    }
    let resolved = resolve_workstream(
        &auth.catalog,
        &auth.access,
        &selection,
        WorkstreamAction::Read,
    )?;
    authorize_query(
        auth,
        Some(resolved.workstream_id),
        resolved.work_item_id.as_deref().or(q.work_id.as_deref()),
        &q.op,
    )?;
    let stream = resolved.workstream_id.to_string();
    let binding = hash(
        &json!({"auth":auth.binding,"snapshot":auth.snapshot,"epoch":auth.epoch,"stream":stream,
        "authority":resolved.authority_version,"grant":auth.grant_versions[&resolved.workstream_id],"op":q.op,
        "work":resolved.work_item_id,"session":q.session_id,"search":q.search}),
    )?;
    let c = cursor(q, &binding)?;
    let limit = i64::from(q.limit.unwrap_or(50));
    let data = match q.op.as_str() {
        "work.observe" => {
            let work = resolved.work_item_id.as_deref().ok_or(PgError::Forbidden)?;
            let (_, ownership) = work_binding(tx, tenant, project, auth, work).await?;
            observation::read(
                tx,
                tenant,
                project,
                auth,
                work,
                &stream,
                ownership,
                q.session_id.as_deref(),
            )
            .await?
        }
        "execution.inspect" => {
            let work = resolved.work_item_id.as_deref().ok_or(PgError::Forbidden)?;
            let (_, ownership) = work_binding(tx, tenant, project, auth, work).await?;
            crate::workstream_command::executions::inspect(
                tx,
                tenant,
                project,
                auth,
                work,
                &stream,
                ownership,
                q.execution_id.as_deref().ok_or(PgError::Forbidden)?,
                q.session_id.as_deref(),
            )
            .await?
        }
        "claim.inspect" => {
            let work = resolved.work_item_id.as_deref().ok_or(PgError::Forbidden)?;
            let (_, ownership) = work_binding(tx, tenant, project, auth, work).await?;
            crate::workstream_command::claims::inspect(
                tx,
                tenant,
                project,
                auth,
                work,
                &stream,
                ownership,
                q.claim_id.as_deref().ok_or(PgError::Forbidden)?,
                q.session_id.as_deref(),
            )
            .await?
        }
        "handoff.inspect" => {
            let work = resolved.work_item_id.as_deref().ok_or(PgError::Forbidden)?;
            let _ = work_binding(tx, tenant, project, auth, work).await?;
            let handoff_id = q.handoff_id.as_deref().ok_or(PgError::Forbidden)?;
            let value =
                crate::workstream_command::handoffs::inspect_query(tx, tenant, project, handoff_id)
                    .await?;
            // Scope: handoff must belong to the selected work.
            if value["handoff"]["work_item_id"] != work {
                return Err(PgError::Forbidden);
            }
            value
        }
        "evidence.inspect" => {
            let work = resolved.work_item_id.as_deref().ok_or(PgError::Forbidden)?;
            let _ = work_binding(tx, tenant, project, auth, work).await?;
            let evidence_id = q.evidence_id.as_deref().ok_or(PgError::Forbidden)?;
            let value = crate::workstream_command::reviews::inspect_evidence(
                tx,
                tenant,
                project,
                evidence_id,
            )
            .await?;
            if value["evidence"]["work_id"] != work {
                return Err(PgError::Forbidden);
            }
            value
        }
        "review.inspect" => {
            let work = resolved.work_item_id.as_deref().ok_or(PgError::Forbidden)?;
            let _ = work_binding(tx, tenant, project, auth, work).await?;
            let round_id = q.review_round_id.as_deref().ok_or(PgError::Forbidden)?;
            let value =
                crate::workstream_command::reviews::inspect_review(tx, tenant, project, round_id)
                    .await?;
            if value["review"]["work_id"] != work {
                return Err(PgError::Forbidden);
            }
            value
        }
        "completion.inspect" => {
            let work = resolved.work_item_id.as_deref().ok_or(PgError::Forbidden)?;
            let _ = work_binding(tx, tenant, project, auth, work).await?;
            crate::workstream_command::reviews::inspect_completion(tx, tenant, project, work)
                .await?
        }
        "delivery.inspect" => {
            let work = resolved.work_item_id.as_deref().ok_or(PgError::Forbidden)?;
            let _ = work_binding(tx, tenant, project, auth, work).await?;
            crate::workstream_command::reviews::inspect_delivery(tx, tenant, project, work).await?
        }
        "command.inspect" => {
            let work = resolved.work_item_id.as_deref().ok_or(PgError::Forbidden)?;
            let (_, ownership) = work_binding(tx, tenant, project, auth, work).await?;
            crate::workstream_command::inspect(
                tx,
                tenant,
                project,
                auth,
                q.request_id.as_deref().ok_or(PgError::Forbidden)?,
                work,
                &stream,
                ownership,
            )
            .await?
        }
        "work.list" | "work.search" => {
            let term = q.search.clone().unwrap_or_default();
            // Filter before both the page and count. Search has no global corpus
            // or rank, so private stream contents cannot affect visible results.
            let count:i64=tx.query_one("SELECT count(*) FROM awr_team.work_contracts c JOIN awr_team.workstream_snapshot_ownership o
                USING(tenant_id,project_id,snapshot_id,scope_id,work_id)
                WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.snapshot_id=$3 AND o.workstream_id=$4
                AND strpos(lower(c.title),lower($5))>0", &[&tenant,&project,&auth.snapshot,&stream,&term]).await?.get(0);
            let rows=tx.query("SELECT c.work_id,c.title,c.contract_hash FROM awr_team.work_contracts c JOIN awr_team.workstream_snapshot_ownership o
                USING(tenant_id,project_id,snapshot_id,scope_id,work_id)
                WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.snapshot_id=$3 AND o.workstream_id=$4 AND c.work_id>$5
                AND strpos(lower(c.title),lower($6))>0 ORDER BY c.work_id LIMIT $7",
                &[&tenant,&project,&auth.snapshot,&stream,&c.key,&term,&(limit+1)]).await?;
            let more = rows.len() > limit as usize;
            let items:Vec<_>=rows.iter().take(limit as usize).map(|r|json!({"work_id":r.get::<_,String>(0),"title":r.get::<_,String>(1),"contract_hash":r.get::<_,String>(2)})).collect();
            let next = if more {
                next_cursor(
                    &binding,
                    items.last().unwrap()["work_id"].as_str().unwrap(),
                    0,
                    -1,
                )
            } else {
                Value::Null
            };
            json!({"items":items,"total":count,"next_cursor":next})
        }
        "work.prepare" => {
            let work = resolved
                .work_item_id
                .as_ref()
                .ok_or_else(|| PgError::Protocol("work required".into()))?;
            let row=tx.query_one("SELECT contract_json,contract_hash FROM awr_team.work_contracts
                WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND scope_id='main' AND work_id=$4",
                &[&tenant,&project,&auth.snapshot,&work]).await?;
            let mut contract: WorkContract =
                serde_json::from_value(row.get(0)).map_err(|_| PgError::SourceDivergence)?;
            let contract_hash: String = row.get(1);
            if contract.work_id.as_str() != work
                || contract.hash().map_err(|_| PgError::SourceDivergence)? != contract_hash
            {
                return Err(PgError::SourceDivergence);
            }
            let allowed=tx.query("SELECT work_id FROM awr_team.workstream_snapshot_ownership
                WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND workstream_id=$4 AND work_id=ANY($5)",
                &[&tenant,&project,&auth.snapshot,&stream,&contract.required_dependencies]).await?;
            let ids: std::collections::BTreeSet<String> =
                allowed.iter().map(|r| r.get(0)).collect();
            let missing = contract
                .required_dependencies
                .iter()
                .any(|d| !ids.contains(d));
            contract.required_dependencies.retain(|d| ids.contains(d));
            contract
                .dependency_acceptance
                .retain(|d, _| ids.contains(d));
            let mut reasons = Vec::new();
            if missing {
                reasons.push("dependency_export_unavailable");
            }
            if contract.goals.is_empty() {
                reasons.push("missing_goals");
            }
            if contract.hard_rules.is_empty() {
                reasons.push("missing_hard_rules");
            }
            let runtime = tx.query_opt("SELECT state,work_version,last_fence,recovery_blocked,selected_completion_id
                FROM awr_team.work_runtime WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3",
                &[&tenant,&project,&work]).await?.map(|r| json!({"state":r.get::<_,String>(0),
                    "work_version":r.get::<_,i64>(1).to_string(),"last_fence":r.get::<_,i64>(2).to_string(),
                    "recovery_blocked":r.get::<_,bool>(3),"selected_completion_id":r.get::<_,Option<String>>(4)}));
            let (_, ownership) = work_binding(tx, tenant, project, auth, work).await?;
            let (required_specs, readable_refs, spec_reasons) =
                controlled_prepare_context(tx, tenant, project, auth, resolved.workstream_id)
                    .await?;
            reasons.extend(spec_reasons);
            let mut data = json!({
                "work_id":work,
                "contract_hash":contract_hash,
                "visible_contract":contract,
                "published_contract":contract,
                "required_specs":required_specs,
                "authorized_readable_refs":readable_refs,
                "runtime":runtime,
                "ownership_version":ownership.to_string(),
                "dependency_export_unavailable":missing,
                "context_complete":reasons.is_empty(),
                "completeness_reasons":reasons,
                "execution_admission":"not_evaluated",
                "next_step": if reasons.is_empty() {
                    Value::Null
                } else {
                    json!("re-prepare after restoring missing authorized specs/deps, or raise max_context_bytes / use source.content")
                }
            });
            // Source snapshots and project audit revisions may advance because
            // of unrelated work. Bind actual selected facts, not those cursors.
            let context_hash = hash(&json!({"domain":"awr-team-workstream-context-v1",
                "reader":auth.binding,"epoch":auth.epoch,"project_status":auth.project_status,
                "workstream":auth.catalog.get(resolved.workstream_id)?,
                "grant_version":auth.grant_versions[&resolved.workstream_id],"data":data}))?;
            data["context_hash"] = json!(context_hash);
            data["context_hash_protocol"] = json!("awr-team-workstream-context-v1");
            // Advice is not consumed context or execution admission. Preserve the
            // original context hash and fit optional advice inside the read budget.
            let observed = observation::read(
                tx,
                tenant,
                project,
                auth,
                work,
                &stream,
                ownership,
                q.session_id.as_deref(),
            )
            .await?;
            let mut hint = observed["guidance"].clone();
            if !reasons.is_empty() {
                hint = guidance::select(&observed, false, false);
            }
            let mut with_hint = data.clone();
            with_hint["guidance"] = hint;
            if serde_json::to_vec(&with_hint)
                .map_err(|_| PgError::SourceDivergence)?
                .len()
                <= q.max_context_bytes.unwrap_or(65536)
            {
                data = with_hint;
            }

            if serde_json::to_vec(&data)
                .map_err(|_| PgError::SourceDivergence)?
                .len()
                > q.max_context_bytes.unwrap_or(65536)
            {
                return Err(PgError::ContextIncomplete);
            }
            data
        }
        "events.list" => {
            let after: i64 = c.revision.parse().map_err(|_| PgError::CursorExpired)?;
            let work = resolved.work_item_id.as_deref();
            let rows=tx.query("SELECT e.id,e.project_revision,e.event_index,e.event_type,e.work_id FROM awr_team.events e
                WHERE e.tenant_id=$1 AND e.project_id=$2 AND e.workstream_id=$3
                AND ($4::text IS NULL OR e.work_id=$4) AND (e.project_revision,e.event_index)>($5,$6)
                AND (e.work_id IS NULL OR EXISTS(SELECT 1 FROM awr_team.workstream_snapshot_ownership o
                    WHERE o.tenant_id=e.tenant_id AND o.project_id=e.project_id AND o.snapshot_id=$8
                    AND o.work_id=e.work_id AND o.workstream_id=e.workstream_id))
                ORDER BY e.project_revision,e.event_index LIMIT $7", &[&tenant,&project,&stream,&work,&after,&c.index,&(limit+1),&auth.snapshot]).await?;
            let more = rows.len() > limit as usize;
            let items:Vec<_>=rows.iter().take(limit as usize).map(|r|json!({"id":r.get::<_,String>(0),"project_revision":r.get::<_,i64>(1).to_string(),
                "event_index":r.get::<_,i32>(2),"event_type":r.get::<_,String>(3),"work_id":r.get::<_,Option<String>>(4)})).collect();
            let next = if more {
                let r = &rows[limit as usize - 1];
                next_cursor(&binding, "", r.get(1), r.get(2))
            } else {
                Value::Null
            };
            json!({"items":items,"next_cursor":next,"payloads_included":false})
        }
        "session.inspect" | "work.recovery" => {
            let work = resolved.work_item_id.as_ref().ok_or(PgError::Forbidden)?;
            let session = q.session_id.as_deref();
            let current_contract: String = tx.query_one("SELECT contract_hash FROM awr_team.work_contracts
                WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND scope_id='main' AND work_id=$4",
                &[&tenant,&project,&auth.snapshot,&work]).await?.get(0);
            let rows=tx.query("SELECT s.id,s.state,s.session_version,c.id,c.context_hash,c.contract_hash,c.next_action,c.open_loops_json
                FROM awr_team.sessions s JOIN awr_team.workstream_snapshot_ownership o
                  ON o.tenant_id=s.tenant_id AND o.project_id=s.project_id AND o.work_id=s.work_id AND o.snapshot_id=$3
                  AND o.workstream_id=s.workstream_id AND o.ownership_version=s.ownership_version AND o.scope_id=s.scope_id
                LEFT JOIN awr_team.checkpoints c ON c.tenant_id=s.tenant_id AND c.project_id=s.project_id
                  AND c.session_id=s.id AND c.id=s.latest_checkpoint_id
                WHERE s.tenant_id=$1 AND s.project_id=$2 AND s.workstream_id=$4 AND s.work_id=$5
                  AND ($6::text IS NULL OR s.id=$6) ORDER BY c.created_at DESC NULLS LAST,s.id DESC LIMIT 2",
                &[&tenant,&project,&auth.snapshot,&stream,&work,&session]).await?;
            let items:Vec<_>=rows.iter().map(|r|json!({"session_id":r.get::<_,String>(0),"state":r.get::<_,String>(1),"session_version":r.get::<_,i64>(2).to_string(),
                "checkpoint_id":r.get::<_,Option<String>>(3),"context_hash":r.get::<_,Option<String>>(4),"contract_hash":r.get::<_,Option<String>>(5),
                "contract_matches_current":r.get::<_,Option<String>>(5).map(|hash| hash==current_contract),
                "next_action":r.get::<_,Option<String>>(6),"open_loops":r.get::<_,Option<Value>>(7)})).collect();
            json!({"items":items,"current_contract_hash":current_contract,"automatic_resume":false})
        }
        "source.content" => {
            let path = q.source_path.as_deref().ok_or(PgError::Forbidden)?;
            read_controlled_source_content(
                tx,
                tenant,
                project,
                auth,
                path,
                q.expected_sha256.as_deref(),
                q.max_context_bytes,
            )
            .await?
        }
        "artifact.content" => {
            let work = resolved.work_item_id.as_deref().ok_or(PgError::Forbidden)?;
            let _ = work_binding(tx, tenant, project, auth, work).await?;
            let artifact_id = q.artifact_id.as_deref().ok_or(PgError::Forbidden)?;
            read_controlled_artifact_content(
                tx,
                tenant,
                project,
                work,
                artifact_id,
                q.expected_sha256.as_deref(),
                q.max_context_bytes,
            )
            .await?
        }
        "planning.outcome" => {
            // Receipt lookup uses the same auth gate as SourceStore; here we
            // only expose the redacted receipt body already stored.
            let request_id = q.request_id.as_deref().ok_or(PgError::Forbidden)?;
            let row = tx
                .query_opt(
                    "SELECT op, request_hash, result_json, created_at::text
                     FROM awr_team.planning_command_receipts
                     WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
                    &[&tenant, &project, &request_id],
                )
                .await?;
            match row {
                Some(r) => json!({
                    "protocol":"awr-team-planning-command-v1",
                    "request_id":request_id,
                    "op":r.get::<_,String>(0),
                    "request_hash":r.get::<_,String>(1),
                    "result":r.get::<_,Value>(2),
                    "created_at":r.get::<_,String>(3),
                    "already_recorded":true,
                    "next_step":"reuse this receipt; do not resubmit with a new request_id"
                }),
                None => json!({
                    "protocol":"awr-team-planning-command-v1",
                    "request_id":request_id,
                    "already_recorded":false,
                    "result":null,
                    "next_step":"absent receipt is unknown — wait/retry inspect before submitting a new request_id"
                }),
            }
        }
        _ => return Err(PgError::Unsupported("workstream query operation".into())),
    };
    Ok(
        json!({"protocol_version":1,"workstream_id":stream,"authority_version":resolved.authority_version.to_string(),"scope_id":"main","selection_basis":resolved.basis,
        "coordinator_epoch":auth.epoch,"project_status":auth.project_status,"source_snapshot_id":auth.snapshot,"project_revision":auth.revision.to_string(),"data":data}),
    )
}

fn safe_relative_source_path(path: &str) -> PgResult<()> {
    if path.is_empty()
        || path.len() > 512
        || path.starts_with('/')
        || path.contains("..")
        || path.contains('\\')
        || path.contains(':')
        || path.contains("://")
        || path.chars().any(char::is_control)
    {
        return Err(PgError::UnsafeSourcePath(path.into()));
    }
    Ok(())
}

async fn active_source_files(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
) -> PgResult<Vec<Value>> {
    let row = tx
        .query_opt(
            "SELECT s.source_ref_json
             FROM awr_team.projects p
             JOIN awr_team.source_snapshots s
               ON s.tenant_id=p.tenant_id AND s.project_id=p.id AND s.id=p.active_snapshot_id
             WHERE p.tenant_id=$1 AND p.id=$2",
            &[&tenant, &project],
        )
        .await?;
    let Some(row) = row else {
        return Ok(Vec::new());
    };
    let source_ref: Value = row.get(0);
    Ok(source_ref
        .get("files")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default())
}

async fn controlled_prepare_context(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    stream_id: awr_core::Id,
) -> PgResult<(Vec<Value>, Vec<Value>, Vec<&'static str>)> {
    let files = active_source_files(tx, tenant, project).await?;
    let mut by_path = std::collections::BTreeMap::new();
    for f in &files {
        if let Some(path) = f.get("path").and_then(|v| v.as_str()) {
            by_path.insert(path.to_string(), f.clone());
        }
    }
    let mut required_specs = Vec::new();
    let mut readable_refs = Vec::new();
    let mut reasons: Vec<&'static str> = Vec::new();
    // Code scope includes directories and files that a contributor has yet to
    // create. Required input documents belong to the selected workstream.
    let stream = auth.catalog.get(stream_id)?;
    for path in &stream.acceptance_contracts {
        if safe_relative_source_path(path).is_err() {
            reasons.push("spec_path_rejected");
            continue;
        }
        match by_path.get(path) {
            Some(file) => {
                let text = file.get("text").and_then(|v| v.as_str()).unwrap_or("");
                // Inline context must obey the same shared-document boundary as
                // source.content; a selected stream cannot expose a private peer.
                if authorize_source_file_streams(auth, path, text).is_err() {
                    reasons.push("required_spec_forbidden");
                    continue;
                }
                let sha = file.get("sha256").and_then(|v| v.as_str()).unwrap_or("");
                let bytes = file.get("bytes").and_then(|v| v.as_u64()).unwrap_or(0);
                let text = file.get("text").and_then(|v| v.as_str()).unwrap_or("");
                // Re-hash text to prevent digest bypass via mutated JSON.
                let digest = crate::source::sha256_hex(text.as_bytes());
                if !sha.is_empty() && digest != sha {
                    reasons.push("spec_digest_mismatch");
                    continue;
                }
                required_specs.push(json!({
                    "path": path,
                    "sha256": digest,
                    "byte_length": bytes,
                    "content_included": bytes <= 16384,
                    "text": if bytes <= 16384 { Value::String(text.to_string()) } else { Value::Null },
                    "next_step": if bytes > 16384 {
                        json!("fetch via source.content with max_context_bytes")
                    } else {
                        Value::Null
                    }
                }));
                readable_refs.push(json!({
                    "kind":"source",
                    "path":path,
                    "sha256":digest,
                    "byte_length":bytes
                }));
            }
            None => {
                reasons.push("required_spec_missing");
                readable_refs.push(json!({
                    "kind":"source",
                    "path":path,
                    "available":false,
                    "next_step":"restore the published source path under the active snapshot; historical versions and external URLs are not accepted"
                }));
            }
        }
    }
    Ok((required_specs, readable_refs, reasons))
}

async fn read_controlled_source_content(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    path: &str,
    expected_sha256: Option<&str>,
    max_bytes: Option<usize>,
) -> PgResult<Value> {
    safe_relative_source_path(path)?;
    let files = active_source_files(tx, tenant, project).await?;
    let file = files
        .into_iter()
        .find(|f| f.get("path").and_then(|v| v.as_str()) == Some(path))
        .ok_or_else(|| {
            PgError::Protocol(
                "source path not present in the active authorized snapshot; historical versions, URLs and attachments cannot bypass project/stream auth".into(),
            )
        })?;
    let text = file
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| PgError::Protocol("source file text unavailable".into()))?;
    // Require read grant over every workstream whose identity/content is exposed.
    // Mixed-scope catalog files (e.g. workstreams.json) are refused unless the
    // caller can read the entire file's stream set — never return a raw shared
    // snapshot based only on nonempty visible streams.
    authorize_source_file_streams(auth, path, text)?;
    let digest = crate::source::sha256_hex(text.as_bytes());
    let recorded = file.get("sha256").and_then(|v| v.as_str()).unwrap_or("");
    if !recorded.is_empty() && recorded != digest {
        return Err(PgError::SnapshotDrift(path.into()));
    }
    if let Some(expected) = expected_sha256 {
        if expected != digest {
            return Err(PgError::SnapshotDrift(path.into()));
        }
    }
    let max = max_bytes.unwrap_or(65536);
    if text.len() > max {
        return Err(PgError::ContextIncomplete);
    }
    let _ = (tenant, project);
    Ok(json!({
        "kind":"source",
        "path":path,
        "sha256":digest,
        "byte_length":text.len(),
        "text":text,
        "snapshot":"active",
        "history_bypass":false,
        "url_bypass":false,
        "next_step":null
    }))
}

fn authorize_source_file_streams(auth: &ReaderAuthority, path: &str, text: &str) -> PgResult<()> {
    use awr_core::Id;
    use std::collections::BTreeSet;

    let mut required: BTreeSet<Id> = BTreeSet::new();
    if path == "workstreams.json" || path.ends_with("/workstreams.json") {
        let bundle: Value = serde_json::from_str(text).map_err(|e| {
            PgError::Protocol(format!("workstreams.json unreadable for auth binding: {e}"))
        })?;
        let streams = bundle
            .pointer("/catalog/workstreams")
            .and_then(Value::as_array)
            .or_else(|| bundle.get("workstreams").and_then(Value::as_array));
        if let Some(streams) = streams {
            for stream in streams {
                if let Some(id_str) = stream.get("id").and_then(Value::as_str) {
                    let id: Id = id_str.parse().map_err(|_| {
                        PgError::Protocol("invalid workstream id in catalog".into())
                    })?;
                    required.insert(id);
                }
            }
        }
        if let Some(contracts) = bundle.get("contracts").and_then(Value::as_array) {
            for c in contracts {
                if let Some(id_str) = c.get("workstream_id").and_then(Value::as_str) {
                    let id: Id = id_str.parse().map_err(|_| {
                        PgError::Protocol("invalid workstream_id in contracts".into())
                    })?;
                    required.insert(id);
                }
            }
        }
    } else {
        // Spec / other snapshot files: only readable when referenced by a
        // workstream the caller can already read.
        for stream in &auth.catalog.workstreams {
            if stream.acceptance_contracts.iter().any(|p| p == path) {
                required.insert(stream.id);
            }
        }
        if required.is_empty() {
            // Unscoped paths in the shared snapshot are not readable via a
            // partial stream grant.
            return Err(PgError::Forbidden);
        }
    }

    if required.is_empty() {
        return Err(PgError::Forbidden);
    }
    for id in &required {
        let Some(grant) = auth.access.grants.iter().find(|g| g.workstream_id == *id) else {
            return Err(PgError::Forbidden);
        };
        if !grant.read {
            return Err(PgError::Forbidden);
        }
    }
    Ok(())
}

async fn read_controlled_artifact_content(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work: &str,
    artifact_id: &str,
    expected_sha256: Option<&str>,
    max_bytes: Option<usize>,
) -> PgResult<Value> {
    // Bind artifact to evidence for the authorized work — refuse free-floating IDs.
    let row = tx
        .query_opt(
            "SELECT a.sha256, a.state, a.content, a.byte_length, a.media_type
             FROM awr_team.artifacts a
             JOIN awr_team.evidence e
               ON e.tenant_id=a.tenant_id AND e.project_id=a.project_id AND e.artifact_id=a.id
             WHERE a.tenant_id=$1 AND a.project_id=$2 AND a.id=$3 AND e.work_id=$4",
            &[&tenant, &project, &artifact_id, &work],
        )
        .await?
        .ok_or(PgError::Forbidden)?;
    let sha: String = row.get(0);
    let state: String = row.get(1);
    let content: Option<Vec<u8>> = row.get(2);
    let byte_length: i64 = row.get(3);
    let media: Option<String> = row.get(4);
    let content = content.ok_or_else(|| {
        PgError::Protocol(
            "artifact content not persisted; metadata-only records cannot be read via MCP".into(),
        )
    })?;
    let digest = crate::source::sha256_hex(&content);
    if state != "finalized" || digest != sha {
        return Err(PgError::SnapshotDrift(artifact_id.into()));
    }
    if let Some(expected) = expected_sha256 {
        if expected != digest {
            return Err(PgError::SnapshotDrift(artifact_id.into()));
        }
    }
    let max = max_bytes.unwrap_or(65536);
    if content.len() > max {
        return Err(PgError::ContextIncomplete);
    }
    let text = String::from_utf8(content.clone()).ok();
    Ok(json!({
        "kind":"artifact",
        "artifact_id":artifact_id,
        "work_id":work,
        "sha256":digest,
        "byte_length":byte_length,
        "media_type":media,
        "text":text,
        "content_base64": if text.is_none() {
            Value::String(base64_encode(&content))
        } else {
            Value::Null
        },
        "path_bypass":false,
        "url_bypass":false,
        "history_bypass":false,
        "next_step":null
    }))
}

fn base64_encode(bytes: &[u8]) -> String {
    // Minimal base64 without extra deps: use a simple alphabet encoder.
    const ALPH: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    let mut i = 0;
    while i < bytes.len() {
        let b0 = bytes[i] as u32;
        let b1 = if i + 1 < bytes.len() {
            bytes[i + 1] as u32
        } else {
            0
        };
        let b2 = if i + 2 < bytes.len() {
            bytes[i + 2] as u32
        } else {
            0
        };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPH[((triple >> 18) & 63) as usize] as char);
        out.push(ALPH[((triple >> 12) & 63) as usize] as char);
        if i + 1 < bytes.len() {
            out.push(ALPH[((triple >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if i + 2 < bytes.len() {
            out.push(ALPH[(triple & 63) as usize] as char);
        } else {
            out.push('=');
        }
        i += 3;
    }
    out
}
