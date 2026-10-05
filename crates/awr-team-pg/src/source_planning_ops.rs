//! Authenticated planning ops for HTTP/MCP (AWR-TMCP-023).
//!
//! Wraps TMCP-021/022 domain entrypoints with stable `request_id` receipts so
//! HTTP and MCP share identity, action auth, and idempotent replay. Clients
//! never receive SQL tools, arbitrary filesystem writes, or direct `done`.

use super::planning::{DraftCandidateCreate, SuggestionSubmit};
use super::writeback::WritebackActivateRequest;
use super::{PgError, PgResult, SoleSourceBinding, SoleSourceKind, SourceStore};
use crate::tx::bind_workstream_scope;
use crate::workstream_auth::authenticate;
use awr_team::DraftChange;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;

const RECEIPT_PROTOCOL: &str = "awr-team-planning-command-v1";

fn planning_protocol_v1() -> u32 {
    1
}

fn require_protocol(v: u32) -> PgResult<()> {
    if v != 1 {
        return Err(PgError::Protocol(
            "planning protocol_version must be 1".into(),
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanningSuggestRequest {
    #[serde(default = "planning_protocol_v1")]
    pub protocol_version: u32,
    pub request_id: String,
    pub rationale: String,
    pub affected_work_keys: Vec<String>,
    #[serde(default)]
    pub proposed_notes: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_person_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanningDraftRequest {
    #[serde(default = "planning_protocol_v1")]
    pub protocol_version: u32,
    pub request_id: String,
    /// `create` or `edit` (split/cancel/archive are DraftChange ops inside `changes`).
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_id: Option<String>,
    #[serde(default)]
    pub changes: Vec<DraftChange>,
    #[serde(default)]
    pub suggestion_ids: Vec<String>,
    #[serde(default)]
    pub allowed_spec_roots: Vec<String>,
    #[serde(default)]
    pub project_goal_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_approve_policy: Option<awr_team::OrdinaryPlanningSelfApprovePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_person_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanningApproveRequest {
    #[serde(default = "planning_protocol_v1")]
    pub protocol_version: u32,
    pub request_id: String,
    pub candidate_id: String,
    pub candidate_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_person_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanningPublishRequest {
    #[serde(default = "planning_protocol_v1")]
    pub protocol_version: u32,
    pub request_id: String,
    pub candidate_id: String,
    pub candidate_digest: String,
    /// When true, also activate writeback using the registered sole source.
    #[serde(default)]
    pub activate: bool,
    /// When activating after a prior publish-only receipt, pass that receipt id
    /// (and omit re-publish). Uses a fresh request_id for the activate receipt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish_receipt_id: Option<String>,
    #[serde(default)]
    pub impact_proven: bool,
    #[serde(default)]
    pub stopped_work_ids: Vec<String>,
}

fn identity(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control)
}

fn require_request_id(id: &str) -> PgResult<()> {
    if !identity(id) {
        return Err(PgError::Protocol(
            "request_id required (1..128, no controls) for idempotent planning ops".into(),
        ));
    }
    Ok(())
}

fn intent_hash(op: &str, body: &Value) -> PgResult<String> {
    awr_team::request_hash(&json!({"protocol": RECEIPT_PROTOCOL, "op": op, "body": body}))
        .map_err(|_| PgError::Protocol("planning request hash failed".into()))
}

fn action_for_planning_op(op: &str) -> PgResult<awr_team::Action> {
    match op {
        "planning.propose" => Ok(awr_team::Action::PlanningPropose),
        "planning.edit_draft" => Ok(awr_team::Action::PlanningEditDraft),
        "planning.approve" => Ok(awr_team::Action::PlanningApprove),
        "planning.publish" | "planning.activate" => Ok(awr_team::Action::PlanningPublish),
        other => Err(PgError::Protocol(format!(
            "unsupported planning op '{other}'"
        ))),
    }
}

enum PlanningReservation {
    Completed(Value),
    /// `placeholder` is the reserved row's result_json, including `domain_id`
    /// and any resume markers stored by the first insert.
    Pending {
        domain_id: String,
        placeholder: Value,
    },
}

impl SourceStore {
    /// One in-flight command per request_id. The receipt commits before the
    /// domain mutation, so a peer must wait until finalize instead of applying
    /// the same edit, approval, or publish again.
    async fn with_planning_command_lock<T, Fut>(
        &self,
        tenant_id: &str,
        project_id: &str,
        request_id: &str,
        body: Fut,
    ) -> PgResult<T>
    where
        Fut: std::future::Future<Output = PgResult<T>>,
    {
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        let lock_key = format!("{tenant_id}\u{1f}{project_id}\u{1f}{request_id}");
        tx.execute(
            "SELECT pg_advisory_xact_lock(hashtext($1)::bigint)",
            &[&lock_key],
        )
        .await?;
        let result = body.await;
        tx.commit().await?;
        result
    }
}

/// Both planning transports and the generic query use the same live authority
/// and completed-only receipt view. Reserved commands remain unknown.
pub(crate) async fn read_planning_command_receipt(
    tx: &tokio_postgres::Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    auth: &mut crate::workstream_auth::ReaderAuthority,
    request_id: &str,
) -> PgResult<Option<Value>> {
    require_request_id(request_id)?;
    bind_workstream_scope(tx, tenant_id, project_id).await?;
    let row = tx
        .query_opt(
            "SELECT op, request_hash, actor_id, client_id, result_json, created_at::text, status
         FROM awr_team.planning_command_receipts
         WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
            &[&tenant_id, &project_id, &request_id],
        )
        .await?;
    let identity = row.as_ref().map(|row| {
        (
            row.get::<_, String>(0),
            row.get::<_, String>(2),
            row.get::<_, String>(3),
        )
    });
    let mut affected = None;
    if let Some(row) = &row {
        if row.get::<_, String>(0) == "planning.propose" {
            let body: Value = row.get(4);
            if row.get::<_, String>(6) == "completed" {
                let suggestion = body
                    .pointer("/result/suggestion_id")
                    .and_then(Value::as_str);
                if let Some(id) = suggestion {
                    let suggestion = tx
                        .query_opt(
                            "SELECT affected_work_keys FROM awr_team.planning_suggestions
                         WHERE tenant_id=$1 AND project_id=$2 AND id=$3
                           AND author_actor_id=$4 AND author_client_id=$5",
                            &[
                                &tenant_id,
                                &project_id,
                                &id,
                                &row.get::<_, String>(2),
                                &row.get::<_, String>(3),
                            ],
                        )
                        .await?;
                    affected = suggestion
                        .and_then(|r| serde_json::from_value::<Vec<String>>(r.get(0)).ok());
                }
            } else {
                affected = body
                    .get("affected_work_keys")
                    .cloned()
                    .and_then(|keys| serde_json::from_value::<Vec<String>>(keys).ok());
            }
        }
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    crate::delegation_auth::authorize_planning_outcome(
        tx,
        auth,
        project_id,
        identity
            .as_ref()
            .map(|(op, actor, client)| (op.as_str(), actor.as_str(), client.as_str())),
        affected.as_deref(),
        now_ms,
    )
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    if row.get::<_, String>(6) != "completed" {
        return Ok(None);
    }
    Ok(Some(json!({
        "protocol": RECEIPT_PROTOCOL,
        "request_id": request_id,
        "op": row.get::<_, String>(0),
        "request_hash": row.get::<_, String>(1),
        "actor_id": row.get::<_, String>(2),
        "client_id": row.get::<_, String>(3),
        "result": row.get::<_, Value>(4),
        "created_at": row.get::<_, String>(5),
        "already_recorded": true,
        "next_step": "reuse this receipt; do not resubmit with a new request_id"
    })))
}

impl SourceStore {
    /// Lookup a prior planning mutation receipt (disconnect recovery).
    pub async fn get_planning_command_receipt(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        request_id: &str,
    ) -> PgResult<Option<Value>> {
        require_request_id(request_id)?;
        let mut client = self.connect().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let mut auth = authenticate(&tx, tenant_id, project_id, bearer).await?;
        let out = read_planning_command_receipt(&tx, tenant_id, project_id, &mut auth, request_id)
            .await?;
        tx.commit().await?;
        Ok(out)
    }

    /// Reserve an idempotency slot before the domain mutation. A completed
    /// receipt is returned as-is. A reserved row yields its original
    /// `domain_id` and placeholder so a crashed command can resume once.
    async fn reserve_planning_receipt(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        request_id: &str,
        op: &str,
        hash: &str,
        domain_id: &str,
        extra: &Value,
    ) -> PgResult<PlanningReservation> {
        let action = action_for_planning_op(op)?;
        let mut client = self.connect().await?;
        crate::check_schema(&client).await?;
        // Read committed so a unique-conflict loser can see the winner's row
        // after rolling back to a savepoint. Repeatable read would keep the
        // pre-insert snapshot and could not replay the concurrent reserve.
        let mut tx = client.transaction().await?;
        let mut auth = authenticate(&tx, tenant_id, project_id, bearer).await?;
        if action == awr_team::Action::PlanningPropose {
            let affected: Vec<String> = serde_json::from_value(
                extra
                    .get("affected_work_keys")
                    .cloned()
                    .unwrap_or_else(|| json!([])),
            )
            .map_err(|_| PgError::Forbidden)?;
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            crate::delegation_auth::authorize_planning_suggestion(
                &tx, &mut auth, project_id, &affected, now_ms,
            )
            .await?;
        } else {
            crate::delegation_auth::authorize_project_action(&tx, &mut auth, project_id, action)
                .await?;
        }
        bind_workstream_scope(&tx, tenant_id, project_id).await?;
        let existing = tx
            .query_opt(
                "SELECT request_hash, status, result_json
                 FROM awr_team.planning_command_receipts
                 WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3
                 FOR UPDATE",
                &[&tenant_id, &project_id, &request_id],
            )
            .await?;
        if let Some(row) = existing {
            let prior_hash: String = row.get(0);
            if prior_hash != hash {
                return Err(PgError::IdempotencyConflict);
            }
            let status: String = row.get(1);
            let prior: Value = row.get(2);
            if status == "completed" {
                let mut prior = prior;
                if let Some(obj) = prior.as_object_mut() {
                    obj.insert("already_recorded".into(), Value::Bool(true));
                }
                tx.commit().await?;
                return Ok(PlanningReservation::Completed(prior));
            }
            let domain = prior
                .get("domain_id")
                .and_then(|v| v.as_str())
                .unwrap_or(domain_id)
                .to_string();
            tx.commit().await?;
            return Ok(PlanningReservation::Pending {
                domain_id: domain,
                placeholder: prior,
            });
        }
        let mut placeholder = json!({
            "protocol": RECEIPT_PROTOCOL,
            "request_id": request_id,
            "op": op,
            "request_hash": hash,
            "status": "reserved",
            "domain_id": domain_id,
            "already_recorded": false
        });
        if let (Some(slot), Some(extra)) = (placeholder.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                slot.insert(key.clone(), value.clone());
            }
        }
        let reserve_sp = tx.savepoint("planning_reserve").await?;
        match reserve_sp
            .execute(
                "INSERT INTO awr_team.planning_command_receipts(
                    tenant_id, project_id, request_id, op, request_hash,
                    actor_id, client_id, status, result_json)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,'reserved',$8)",
                &[
                    &tenant_id,
                    &project_id,
                    &request_id,
                    &op,
                    &hash,
                    &auth.actor_id,
                    &auth.client_id,
                    &placeholder,
                ],
            )
            .await
        {
            Ok(_) => {
                reserve_sp.commit().await?;
            }
            Err(e) if e.code() == Some(&tokio_postgres::error::SqlState::UNIQUE_VIOLATION) => {
                // The failed insert aborts the transaction unless we roll back
                // to the savepoint first. Then the committed winner is visible.
                reserve_sp.rollback().await?;
                let row = tx
                    .query_one(
                        "SELECT request_hash, status, result_json
                         FROM awr_team.planning_command_receipts
                         WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
                        &[&tenant_id, &project_id, &request_id],
                    )
                    .await?;
                let prior_hash: String = row.get(0);
                if prior_hash != hash {
                    return Err(PgError::IdempotencyConflict);
                }
                let status: String = row.get(1);
                let prior: Value = row.get(2);
                tx.commit().await?;
                if status == "completed" {
                    let mut prior = prior;
                    if let Some(obj) = prior.as_object_mut() {
                        obj.insert("already_recorded".into(), Value::Bool(true));
                    }
                    return Ok(PlanningReservation::Completed(prior));
                }
                let domain = prior
                    .get("domain_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or(domain_id)
                    .to_string();
                return Ok(PlanningReservation::Pending {
                    domain_id: domain,
                    placeholder: prior,
                });
            }
            Err(e) => return Err(e.into()),
        }
        tx.commit().await?;
        Ok(PlanningReservation::Pending {
            domain_id: domain_id.to_string(),
            placeholder,
        })
    }

    async fn finalize_planning_receipt(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        request_id: &str,
        op: &str,
        hash: &str,
        result: Value,
    ) -> PgResult<Value> {
        let mut client = self.connect().await?;
        // Read committed: two finishes of the same reserved receipt must not
        // turn the second UPDATE into a serialization failure.
        let tx = client.transaction().await?;
        let _auth = authenticate(&tx, tenant_id, project_id, bearer).await?;
        bind_workstream_scope(&tx, tenant_id, project_id).await?;
        let wrapped = json!({
            "protocol": RECEIPT_PROTOCOL,
            "request_id": request_id,
            "op": op,
            "request_hash": hash,
            "result": result,
            "already_recorded": false
        });
        let updated = tx
            .execute(
                "UPDATE awr_team.planning_command_receipts
                 SET status='completed', result_json=$4, updated_at=clock_timestamp()
                 WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3
                   AND request_hash=$5",
                &[&tenant_id, &project_id, &request_id, &wrapped, &hash],
            )
            .await?;
        if updated == 0 {
            let row = tx
                .query_opt(
                    "SELECT request_hash, status, result_json
                     FROM awr_team.planning_command_receipts
                     WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
                    &[&tenant_id, &project_id, &request_id],
                )
                .await?
                .ok_or_else(|| {
                    PgError::Protocol("planning receipt missing after finalize".into())
                })?;
            let prior_hash: String = row.get(0);
            if prior_hash != hash {
                return Err(PgError::IdempotencyConflict);
            }
            let status: String = row.get(1);
            if status != "completed" {
                return Err(PgError::Protocol(
                    "planning receipt finalize lost the reserved row".into(),
                ));
            }
            let prior: Value = row.get(2);
            tx.commit().await?;
            return Ok(prior);
        }
        tx.commit().await?;
        Ok(wrapped)
    }

    pub async fn planning_suggest(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        req: &PlanningSuggestRequest,
    ) -> PgResult<Value> {
        require_request_id(&req.request_id)?;
        self.with_planning_command_lock(
            tenant_id,
            project_id,
            &req.request_id,
            self.planning_suggest_locked(tenant_id, project_id, bearer, req),
        )
        .await
    }

    async fn planning_suggest_locked(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        req: &PlanningSuggestRequest,
    ) -> PgResult<Value> {
        let intent = serde_json::to_value(req).map_err(|e| PgError::Protocol(e.to_string()))?;
        require_request_id(&req.request_id)?;
        require_protocol(req.protocol_version)?;
        let hash = intent_hash("planning.propose", &intent)?;
        let domain_id = crate::tx::new_id();
        let reserved = self
            .reserve_planning_receipt(
                tenant_id,
                project_id,
                bearer,
                &req.request_id,
                "planning.propose",
                &hash,
                &domain_id,
                &json!({"affected_work_keys": req.affected_work_keys}),
            )
            .await?;
        let domain_id = match reserved {
            PlanningReservation::Completed(existing) => return Ok(existing),
            PlanningReservation::Pending { domain_id, .. } => domain_id,
        };
        let submit = SuggestionSubmit {
            rationale: req.rationale.clone(),
            affected_work_keys: req.affected_work_keys.clone(),
            proposed_notes: req.proposed_notes.clone(),
            author_person_id: req.author_person_id.clone(),
            predetermined_suggestion_id: Some(domain_id),
        };
        let bind = crate::source::planning::PlanningCommandBind {
            request_id: req.request_id.clone(),
            op: "planning.propose".into(),
            request_hash: hash.clone(),
        };
        let result = self
            .submit_planning_suggestion_bound(tenant_id, project_id, bearer, &submit, Some(&bind))
            .await?;
        self.finalize_planning_receipt(
            tenant_id,
            project_id,
            bearer,
            &req.request_id,
            "planning.propose",
            &hash,
            result,
        )
        .await
    }

    pub async fn planning_draft(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        req: &PlanningDraftRequest,
    ) -> PgResult<Value> {
        require_request_id(&req.request_id)?;
        self.with_planning_command_lock(
            tenant_id,
            project_id,
            &req.request_id,
            self.planning_draft_locked(tenant_id, project_id, bearer, req),
        )
        .await
    }

    async fn planning_draft_locked(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        req: &PlanningDraftRequest,
    ) -> PgResult<Value> {
        let intent = serde_json::to_value(req).map_err(|e| PgError::Protocol(e.to_string()))?;
        require_request_id(&req.request_id)?;
        require_protocol(req.protocol_version)?;
        let hash = intent_hash("planning.edit_draft", &intent)?;
        let domain_id = req
            .candidate_id
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(crate::tx::new_id);
        let mut extra = json!({});
        if req.mode == "edit" {
            let revision = self
                .candidate_draft_revision(tenant_id, project_id, bearer, &domain_id)
                .await?;
            extra["pre_revision"] = json!(revision);
        }
        let reserved = self
            .reserve_planning_receipt(
                tenant_id,
                project_id,
                bearer,
                &req.request_id,
                "planning.edit_draft",
                &hash,
                &domain_id,
                &extra,
            )
            .await?;
        let (domain_id, placeholder) = match reserved {
            PlanningReservation::Completed(existing) => return Ok(existing),
            PlanningReservation::Pending {
                domain_id,
                placeholder,
            } => (domain_id, placeholder),
        };
        if req.mode == "edit" {
            if let Some(replay) = self
                .replay_applied_draft_edit(
                    tenant_id,
                    project_id,
                    bearer,
                    &domain_id,
                    &placeholder,
                    &req.changes,
                )
                .await?
            {
                return self
                    .finalize_planning_receipt(
                        tenant_id,
                        project_id,
                        bearer,
                        &req.request_id,
                        "planning.edit_draft",
                        &hash,
                        replay,
                    )
                    .await;
            }
        }
        let result = match req.mode.as_str() {
            "create" => {
                let create = DraftCandidateCreate {
                    changes: req.changes.clone(),
                    suggestion_ids: req.suggestion_ids.clone(),
                    allowed_spec_roots: req.allowed_spec_roots.clone(),
                    project_goal_keys: req.project_goal_keys.clone(),
                    self_approve_policy: req.self_approve_policy.clone(),
                    author_person_id: req.author_person_id.clone(),
                    predetermined_candidate_id: Some(domain_id.clone()),
                };
                let bind = crate::source::planning::PlanningCommandBind {
                    request_id: req.request_id.clone(),
                    op: "planning.edit_draft".into(),
                    request_hash: hash.clone(),
                };
                self.create_planning_candidate_bound(
                    tenant_id,
                    project_id,
                    bearer,
                    &create,
                    Some(&bind),
                )
                .await?
            }
            "edit" => {
                let candidate_id = req.candidate_id.as_deref().ok_or_else(|| {
                    PgError::Protocol("candidate_id required for draft edit".into())
                })?;
                self.edit_planning_candidate(
                    tenant_id,
                    project_id,
                    bearer,
                    candidate_id,
                    req.changes.clone(),
                )
                .await?
            }
            other => {
                return Err(PgError::Protocol(format!(
                    "unsupported draft mode '{other}'; use create or edit (split/cancel/archive via changes[].op)"
                )));
            }
        };
        self.finalize_planning_receipt(
            tenant_id,
            project_id,
            bearer,
            &req.request_id,
            "planning.edit_draft",
            &hash,
            result,
        )
        .await
    }

    pub async fn planning_approve(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        req: &PlanningApproveRequest,
    ) -> PgResult<Value> {
        require_request_id(&req.request_id)?;
        self.with_planning_command_lock(
            tenant_id,
            project_id,
            &req.request_id,
            self.planning_approve_locked(tenant_id, project_id, bearer, req),
        )
        .await
    }

    async fn planning_approve_locked(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        req: &PlanningApproveRequest,
    ) -> PgResult<Value> {
        let intent = serde_json::to_value(req).map_err(|e| PgError::Protocol(e.to_string()))?;
        require_request_id(&req.request_id)?;
        require_protocol(req.protocol_version)?;
        let hash = intent_hash("planning.approve", &intent)?;
        let reserved = self
            .reserve_planning_receipt(
                tenant_id,
                project_id,
                bearer,
                &req.request_id,
                "planning.approve",
                &hash,
                &req.candidate_id,
                &json!({}),
            )
            .await?;
        if let PlanningReservation::Completed(existing) = reserved {
            return Ok(existing);
        }
        let result = if let Some(existing) = self
            .existing_approval_result(
                tenant_id,
                project_id,
                bearer,
                &req.candidate_id,
                &req.candidate_digest,
            )
            .await?
        {
            existing
        } else {
            self.approve_planning_candidate(
                tenant_id,
                project_id,
                bearer,
                &req.candidate_id,
                &req.candidate_digest,
                req.author_person_id.as_deref(),
            )
            .await?
        };
        self.finalize_planning_receipt(
            tenant_id,
            project_id,
            bearer,
            &req.request_id,
            "planning.approve",
            &hash,
            result,
        )
        .await
    }

    pub async fn planning_publish(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        req: &PlanningPublishRequest,
    ) -> PgResult<Value> {
        require_request_id(&req.request_id)?;
        self.with_planning_command_lock(
            tenant_id,
            project_id,
            &req.request_id,
            self.planning_publish_locked(tenant_id, project_id, bearer, req),
        )
        .await
    }

    async fn planning_publish_locked(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        req: &PlanningPublishRequest,
    ) -> PgResult<Value> {
        let intent = serde_json::to_value(req).map_err(|e| PgError::Protocol(e.to_string()))?;
        require_request_id(&req.request_id)?;
        require_protocol(req.protocol_version)?;
        let op = if req.activate {
            "planning.activate"
        } else {
            "planning.publish"
        };
        let hash = intent_hash(op, &intent)?;
        let reserved = self
            .reserve_planning_receipt(
                tenant_id,
                project_id,
                bearer,
                &req.request_id,
                op,
                &hash,
                &req.candidate_id,
                &json!({}),
            )
            .await?;
        if let PlanningReservation::Completed(existing) = reserved {
            return Ok(existing);
        }
        let result = if req.activate {
            let receipt_id = if let Some(id) = req.publish_receipt_id.as_deref() {
                id.to_string()
            } else {
                let published = self
                    .publish_or_existing(
                        tenant_id,
                        project_id,
                        bearer,
                        &req.candidate_id,
                        &req.candidate_digest,
                    )
                    .await?;
                published["receipt_id"]
                    .as_str()
                    .ok_or_else(|| PgError::Protocol("publish receipt missing".into()))?
                    .to_string()
            };
            let activated = self
                .activate_planning_writeback_registered(
                    tenant_id,
                    project_id,
                    bearer,
                    &req.request_id,
                    &receipt_id,
                    req.impact_proven,
                    &req.stopped_work_ids,
                )
                .await?;
            json!({
                "publish_receipt_id": receipt_id,
                "activation": activated,
                "next_step": null
            })
        } else {
            let published = self
                .publish_or_existing(
                    tenant_id,
                    project_id,
                    bearer,
                    &req.candidate_id,
                    &req.candidate_digest,
                )
                .await?;
            json!({
                "publish": published,
                "activation": null,
                "next_step": "after disconnect query planning.outcome with this request_id; to activate later call publish with activate=true, publish_receipt_id from this result, and a new request_id after stop/reconcile"
            })
        };
        self.finalize_planning_receipt(
            tenant_id,
            project_id,
            bearer,
            &req.request_id,
            op,
            &hash,
            result,
        )
        .await
    }

    /// Activate using the project's registered sole source — never a client path/URL.
    pub async fn activate_planning_writeback_registered(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        request_id: &str,
        publish_receipt_id: &str,
        impact_proven: bool,
        stopped_work_ids: &[String],
    ) -> PgResult<Value> {
        let binding = self
            .load_registered_sole_source(tenant_id, project_id, bearer)
            .await?;
        match binding.kind {
            SoleSourceKind::ServerDirectory => {
                let ledger = binding
                    .ledger_relative_path
                    .clone()
                    .unwrap_or_else(|| "ledger.yaml".into());
                if ledger.contains("..")
                    || ledger.starts_with('/')
                    || ledger.contains('\\')
                    || ledger.contains(':')
                {
                    return Err(PgError::UnsafeSourcePath(ledger));
                }
                let req = WritebackActivateRequest {
                    request_id: request_id.into(),
                    publish_receipt_id: publish_receipt_id.into(),
                    source_root: PathBuf::from(&binding.locator),
                    ledger_relative_path: ledger,
                    impact_proven,
                    stopped_work_ids: stopped_work_ids.to_vec(),
                };
                self.activate_planning_writeback(tenant_id, project_id, bearer, &req)
                    .await
            }
            SoleSourceKind::PrivateManagementRepo => Err(PgError::Unsupported(
                "private management repo writeback adapter is not enabled for MCP/HTTP; register a server_directory sole source or use the operator-local writeback path".into(),
            )),
        }
    }

    async fn candidate_draft_revision(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        candidate_id: &str,
    ) -> PgResult<i32> {
        let mut client = self.connect().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let mut auth = authenticate(&tx, tenant_id, project_id, bearer).await?;
        crate::delegation_auth::authorize_project_action(
            &tx,
            &mut auth,
            project_id,
            awr_team::Action::PlanningEditDraft,
        )
        .await?;
        bind_workstream_scope(&tx, tenant_id, project_id).await?;
        let row = tx
            .query_opt(
                "SELECT draft_revision FROM awr_team.planning_candidates
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant_id, &project_id, &candidate_id],
            )
            .await?
            .ok_or_else(|| PgError::Protocol("planning candidate not found".into()))?;
        let revision: i32 = row.get(0);
        tx.commit().await?;
        Ok(revision)
    }

    /// A crashed edit already replaced `changes` and bumped revision. Applying
    /// the same request again would bump a second time.
    async fn replay_applied_draft_edit(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        candidate_id: &str,
        placeholder: &Value,
        changes: &[DraftChange],
    ) -> PgResult<Option<Value>> {
        let Some(pre_revision) = placeholder
            .get("pre_revision")
            .and_then(Value::as_i64)
            .and_then(|n| i32::try_from(n).ok())
        else {
            return Err(PgError::Protocol(
                "reserved draft edit is missing pre_revision; refusing to apply it twice".into(),
            ));
        };
        let requested =
            serde_json::to_value(changes).map_err(|e| PgError::Protocol(e.to_string()))?;
        let mut client = self.connect().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let mut auth = authenticate(&tx, tenant_id, project_id, bearer).await?;
        crate::delegation_auth::authorize_project_action(
            &tx,
            &mut auth,
            project_id,
            awr_team::Action::PlanningEditDraft,
        )
        .await?;
        bind_workstream_scope(&tx, tenant_id, project_id).await?;
        let row = tx
            .query_opt(
                "SELECT draft_revision, candidate_digest, state, (changes_json = $4::jsonb)
                 FROM awr_team.planning_candidates
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant_id, &project_id, &candidate_id, &requested],
            )
            .await?
            .ok_or_else(|| PgError::Protocol("planning candidate not found".into()))?;
        let revision: i32 = row.get(0);
        let digest: String = row.get(1);
        let state: String = row.get(2);
        let same_changes: bool = row.get(3);
        tx.commit().await?;
        if same_changes && revision > pre_revision {
            return Ok(Some(json!({
                "candidate_id": candidate_id,
                "draft_revision": revision,
                "candidate_digest": digest,
                "state": state,
                "prior_approval_cleared": state == "drafting",
                "resumed": true
            })));
        }
        if revision == pre_revision {
            return Ok(None);
        }
        Err(PgError::Protocol(
            "planning draft changed during an incomplete edit; inspect the candidate before using a new request_id".into(),
        ))
    }

    async fn existing_approval_result(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        candidate_id: &str,
        candidate_digest: &str,
    ) -> PgResult<Option<Value>> {
        let mut client = self.connect().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let mut auth = authenticate(&tx, tenant_id, project_id, bearer).await?;
        crate::delegation_auth::authorize_project_action(
            &tx,
            &mut auth,
            project_id,
            awr_team::Action::PlanningApprove,
        )
        .await?;
        bind_workstream_scope(&tx, tenant_id, project_id).await?;
        let row = tx
            .query_opt(
                "SELECT a.id, a.draft_revision, a.self_approved, c.delivery_completion_policy
                 FROM awr_team.planning_approvals a
                 JOIN awr_team.planning_candidates c
                   ON c.tenant_id=a.tenant_id AND c.project_id=a.project_id AND c.id=a.candidate_id
                 WHERE a.tenant_id=$1 AND a.project_id=$2 AND a.candidate_id=$3
                   AND a.candidate_digest=$4 AND c.state='approved'
                 ORDER BY a.decided_at DESC
                 LIMIT 1",
                &[&tenant_id, &project_id, &candidate_id, &candidate_digest],
            )
            .await?;
        let Some(row) = row else {
            tx.commit().await?;
            return Ok(None);
        };
        let approval_id: String = row.get(0);
        let draft_revision: i32 = row.get(1);
        let self_approved: bool = row.get(2);
        let delivery_policy: String = row.get(3);
        tx.commit().await?;
        Ok(Some(json!({
            "approval_id": approval_id,
            "candidate_id": candidate_id,
            "candidate_digest": candidate_digest,
            "draft_revision": draft_revision,
            "self_approved": self_approved,
            "state": "approved",
            "delivery_completion_policy": delivery_policy,
            "independent_review_downgraded": false,
            "resumed": true
        })))
    }

    async fn publish_or_existing(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        candidate_id: &str,
        candidate_digest: &str,
    ) -> PgResult<Value> {
        match self
            .publish_planning_candidate(
                tenant_id,
                project_id,
                bearer,
                candidate_id,
                candidate_digest,
            )
            .await
        {
            Ok(published) => Ok(published),
            Err(PgError::CandidateNotApproved) => self
                .existing_publish_result(
                    tenant_id,
                    project_id,
                    bearer,
                    candidate_id,
                    candidate_digest,
                )
                .await?
                .ok_or(PgError::CandidateNotApproved),
            Err(error) => Err(error),
        }
    }

    async fn existing_publish_result(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        candidate_id: &str,
        candidate_digest: &str,
    ) -> PgResult<Option<Value>> {
        let mut client = self.connect().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let mut auth = authenticate(&tx, tenant_id, project_id, bearer).await?;
        crate::delegation_auth::authorize_project_action(
            &tx,
            &mut auth,
            project_id,
            awr_team::Action::PlanningPublish,
        )
        .await?;
        bind_workstream_scope(&tx, tenant_id, project_id).await?;
        let row = tx
            .query_opt(
                "SELECT id, draft_revision, approval_id, source_writeback_pending
                 FROM awr_team.planning_publish_receipts
                 WHERE tenant_id=$1 AND project_id=$2 AND candidate_id=$3 AND candidate_digest=$4
                 ORDER BY created_at DESC
                 LIMIT 1",
                &[&tenant_id, &project_id, &candidate_id, &candidate_digest],
            )
            .await?;
        let Some(row) = row else {
            tx.commit().await?;
            return Ok(None);
        };
        let receipt_id: String = row.get(0);
        let draft_revision: i32 = row.get(1);
        let approval_id: String = row.get(2);
        let pending: bool = row.get(3);
        tx.commit().await?;
        Ok(Some(json!({
            "receipt_id": receipt_id,
            "candidate_id": candidate_id,
            "candidate_digest": candidate_digest,
            "draft_revision": draft_revision,
            "approval_id": approval_id,
            "state": "published",
            "source_writeback_pending": pending,
            "source_bytes_written": !pending,
            "resumed": true
        })))
    }

    async fn load_registered_sole_source(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
    ) -> PgResult<SoleSourceBinding> {
        let mut client = self.connect().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let mut auth = authenticate(&tx, tenant_id, project_id, bearer).await?;
        crate::delegation_auth::authorize_project_action(
            &tx,
            &mut auth,
            project_id,
            awr_team::Action::PlanningPublish,
        )
        .await?;
        let row = tx
            .query_opt(
                "SELECT s.source_ref_json
                 FROM awr_team.projects p
                 JOIN awr_team.source_snapshots s
                   ON s.tenant_id=p.tenant_id AND s.project_id=p.id
                  AND s.id=p.active_snapshot_id
                 WHERE p.tenant_id=$1 AND p.id=$2",
                &[&tenant_id, &project_id],
            )
            .await?
            .ok_or_else(|| {
                PgError::Protocol(
                    "no active source snapshot; ingest and activate a sole-source package first"
                        .into(),
                )
            })?;
        let source_ref: Value = row.get(0);
        let sole = source_ref
            .get("sole_source")
            .cloned()
            .ok_or_else(|| PgError::Protocol("active source lacks sole_source binding".into()))?;
        if sole.is_null() {
            return Err(PgError::Protocol(
                "active source lacks sole_source binding".into(),
            ));
        }
        let binding: SoleSourceBinding = serde_json::from_value(sole)
            .map_err(|e| PgError::Protocol(format!("invalid registered sole_source: {e}")))?;
        tx.commit().await?;
        Ok(binding)
    }
}
