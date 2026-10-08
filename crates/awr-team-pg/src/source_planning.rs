//! Planning suggestions and controlled draft candidates (AWR-TMCP-021).
//!
//! Domain entry methods authenticate the bearer and authorize TMCP-010
//! `planning.*` actions. Suggestions never claim or mutate live deps/acceptance.
//! Approve and publish are separate, digest-bound actions. Source writeback of
//! published candidates is provided by TMCP-022 (`activate_planning_writeback`).
use super::{PgError, PgResult, SourceStore, sha256_hex};
use crate::tx::new_id;
use crate::workstream_auth::{
    ReaderAuthority, authenticate, authenticate_writer, authorize_domain_action,
};
use awr_core::{Id, WorkstreamAction};
use awr_team::{
    AffectedTaskImpact, BaselineView, CandidateState, DraftChange,
    OrdinaryPlanningSelfApprovePolicy, PLANNING_CODEC, PLANNING_CODEC_V2, PLANNING_CODEC_V3,
    PLANNING_CODEC_V4, PLANNING_CODEC_V5, PLANNING_CODEC_V6, PlanningApproval, PlanningCandidate,
    PlanningSuggestion, ResourceRef, SUGGESTION_ADDS_FORMAL_WORK, SUGGESTION_CLAIMABLE,
    SuggestionState, attested_actor_person, authorize_planning_approve, authorize_planning_publish,
    build_candidate_diff, edit_candidate, ensure_independent_review_not_downgraded,
    planning_codec_for_changes, refuse_reader_suggestion_write, validate_candidate,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_postgres::Transaction;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuggestionSubmit {
    pub rationale: String,
    pub affected_work_keys: Vec<String>,
    #[serde(default)]
    pub proposed_notes: Value,
    /// Must match the authenticated actor when present. Not a separate credential.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_person_id: Option<String>,
    /// When set (TMCP-023 receipt resume), reuse this suggestion identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predetermined_suggestion_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftCandidateCreate {
    pub changes: Vec<DraftChange>,
    #[serde(default)]
    pub suggestion_ids: Vec<String>,
    #[serde(default)]
    pub allowed_spec_roots: Vec<String>,
    #[serde(default)]
    pub project_goal_keys: Vec<String>,
    #[serde(default)]
    pub self_approve_policy: Option<OrdinaryPlanningSelfApprovePolicy>,
    /// Must match the authenticated actor when present. Not a separate credential.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_person_id: Option<String>,
    /// When set (TMCP-023 receipt resume), reuse this candidate identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predetermined_candidate_id: Option<String>,
}

/// Optional MCP idempotency receipt bound in the same TX as the planning write (TMCP-040).
#[derive(Clone, Debug)]
pub struct PlanningCommandBind {
    pub request_id: String,
    pub op: String,
    pub request_hash: String,
}

const PLANNING_RECEIPT_PROTOCOL: &str = "awr-planning-command-receipt-v1";

async fn bind_planning_receipt_and_ops_audit(
    tx: &tokio_postgres::Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    auth: &crate::workstream_auth::ReaderAuthority,
    bind: Option<&PlanningCommandBind>,
    action: &str,
    target_kind: &str,
    target_id: Option<&str>,
    change_id: Option<&str>,
    source_version: Option<&str>,
    summary: serde_json::Value,
    result: &serde_json::Value,
) -> PgResult<serde_json::Value> {
    let digest = crate::ops_audit::digest_of(&summary);
    let mut out = result.clone();
    if let Some(b) = bind {
        // The MCP command layer reserves the receipt before the domain write
        // and finalizes it after. Insert here only when this call owns the row.
        let existing = tx
            .query_opt(
                "SELECT 1 FROM awr_team.planning_command_receipts
                 WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
                &[&tenant_id, &project_id, &b.request_id],
            )
            .await?;
        if existing.is_none() {
            let wrapped = serde_json::json!({
                "protocol": PLANNING_RECEIPT_PROTOCOL,
                "request_id": b.request_id,
                "op": b.op,
                "request_hash": b.request_hash,
                "result": result,
                "already_recorded": false
            });
            tx.execute(
                "INSERT INTO awr_team.planning_command_receipts(
                    tenant_id, project_id, request_id, op, request_hash,
                    actor_id, client_id, result_json)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
                &[
                    &tenant_id,
                    &project_id,
                    &b.request_id,
                    &b.op,
                    &b.request_hash,
                    &auth.actor_id,
                    &auth.client_id,
                    &wrapped,
                ],
            )
            .await?;
            out = wrapped;
        }
    }
    let already_audited = if let Some(b) = bind {
        tx.query_opt(
            "SELECT 1 FROM awr_team.ops_audit_records
             WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3 AND action=$4",
            &[&tenant_id, &project_id, &b.request_id, &action],
        )
        .await?
        .is_some()
    } else {
        false
    };
    if already_audited {
        return Ok(out);
    }
    let mut audit = crate::ops_audit::write_from_auth(
        auth,
        crate::ops_audit::OpsCategory::Planning,
        action,
        target_kind,
    );
    audit.target_id = target_id.map(|s| s.to_string());
    audit.change_id = change_id.map(|s| s.to_string());
    audit.request_id = bind.map(|b| b.request_id.clone());
    audit.source_version = source_version.map(|s| s.to_string());
    audit.digest = Some(digest);
    audit.summary = summary;
    crate::ops_audit::record_in_tx(tx, tenant_id, project_id, &audit).await?;
    Ok(out)
}

fn map_team(err: awr_team::TeamError) -> PgError {
    match err {
        awr_team::TeamError::PermissionDenied(msg) => {
            if msg.contains("downgraded") {
                PgError::PolicyDowngrade
            } else {
                PgError::Forbidden
            }
        }
        other => PgError::Protocol(other.to_string()),
    }
}

fn json_string_list(v: &Value) -> PgResult<Vec<String>> {
    v.as_array()
        .ok_or_else(|| PgError::Protocol("expected json string array".into()))?
        .iter()
        .map(|x| {
            x.as_str()
                .map(str::to_owned)
                .ok_or_else(|| PgError::Protocol("expected string in json array".into()))
        })
        .collect()
}

impl SourceStore {
    /// Submit a planning suggestion (`planning.propose`). Not claimable; does
    /// not add formal work or mutate live deps/acceptance. Readers are refused.
    pub async fn submit_planning_suggestion(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        submit: &SuggestionSubmit,
    ) -> PgResult<Value> {
        self.submit_planning_suggestion_bound(tenant_id, project_id, bearer, submit, None)
            .await
    }

    pub async fn submit_planning_suggestion_bound(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        submit: &SuggestionSubmit,
        bind: Option<&PlanningCommandBind>,
    ) -> PgResult<Value> {
        let mut client = self.connect().await?;
        crate::check_schema(&client).await?;
        // Read committed: a concurrent resume of the same predetermined id uses
        // ON CONFLICT DO NOTHING, and repeatable read turns that into a
        // serialization failure instead of revealing the committed row.
        let tx = client.transaction().await?;
        let mut auth = authenticate_writer(&tx, tenant_id, project_id, bearer).await?;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        crate::delegation_auth::authorize_planning_suggestion(
            &tx,
            &mut auth,
            project_id,
            &submit.affected_work_keys,
            now_ms,
        )
        .await?;
        let scope = crate::workstream_auth::authority_scope(&auth, None, None);
        refuse_reader_suggestion_write(&scope).map_err(map_team)?;
        if SUGGESTION_CLAIMABLE || SUGGESTION_ADDS_FORMAL_WORK {
            return Err(PgError::Protocol(
                "suggestion invariants broken: must not be claimable or formal work".into(),
            ));
        }
        let (baseline_digest, baseline_epoch) =
            current_baseline(&tx, tenant_id, project_id).await?;
        let suggestion_id = submit
            .predetermined_suggestion_id
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(new_id);
        let person = attested_actor_person(&auth.actor_id, submit.author_person_id.as_deref())
            .map_err(map_team)?;
        let suggestion = PlanningSuggestion {
            codec: PLANNING_CODEC.into(),
            suggestion_id: suggestion_id.clone(),
            project_id: project_id.into(),
            author_person_id: person.clone(),
            author_actor_id: auth.actor_id.clone(),
            rationale: submit.rationale.clone(),
            version: 1,
            baseline_digest: baseline_digest.clone(),
            baseline_epoch: baseline_epoch.clone(),
            affected_work_keys: submit.affected_work_keys.clone(),
            proposed_notes: if submit.proposed_notes.is_null() {
                json!({})
            } else {
                submit.proposed_notes.clone()
            },
            state: SuggestionState::Open,
        };
        suggestion.validate().map_err(map_team)?;
        let keys = serde_json::to_value(&suggestion.affected_work_keys)
            .map_err(|e| PgError::Protocol(e.to_string()))?;
        let inserted = tx
            .execute(
                "INSERT INTO awr_team.planning_suggestions(
                    tenant_id, project_id, id, author_person_id, author_actor_id, author_client_id,
                    rationale, version, baseline_digest, baseline_epoch, affected_work_keys,
                    proposed_notes, state)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,'open')
                 ON CONFLICT (tenant_id, project_id, id) DO NOTHING",
                &[
                    &tenant_id,
                    &project_id,
                    &suggestion_id,
                    &suggestion.author_person_id,
                    &auth.actor_id,
                    &auth.client_id,
                    &suggestion.rationale,
                    &(suggestion.version as i32),
                    &baseline_digest,
                    &baseline_epoch,
                    &keys,
                    &suggestion.proposed_notes,
                ],
            )
            .await?;
        // On resume, reload the durable row so callers always see one identity.
        let row = if inserted == 0 {
            tx.query_one(
                "SELECT id, version, author_person_id, author_actor_id, rationale,
                        baseline_digest, baseline_epoch, state
                 FROM awr_team.planning_suggestions
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant_id, &project_id, &suggestion_id],
            )
            .await?
        } else {
            // Synthesize from just-inserted values via a round-trip for uniformity.
            tx.query_one(
                "SELECT id, version, author_person_id, author_actor_id, rationale,
                        baseline_digest, baseline_epoch, state
                 FROM awr_team.planning_suggestions
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant_id, &project_id, &suggestion_id],
            )
            .await?
        };
        let result = json!({
            "suggestion_id": row.get::<_, String>(0),
            "version": row.get::<_, i32>(1),
            "author_person_id": row.get::<_, String>(2),
            "author_actor_id": row.get::<_, String>(3),
            "rationale": row.get::<_, String>(4),
            "baseline_digest": row.get::<_, String>(5),
            "baseline_epoch": row.get::<_, String>(6),
            "claimable": false,
            "adds_formal_work": false,
            "mutates_live_deps": false,
            "mutates_live_acceptance": false,
            "state": row.get::<_, String>(7),
        });
        let summary = json!({
            "suggestion_id": suggestion_id,
            "baseline_digest": baseline_digest,
            "affected_work_keys": suggestion.affected_work_keys,
        });
        let out = bind_planning_receipt_and_ops_audit(
            &tx,
            tenant_id,
            project_id,
            &auth,
            bind,
            "planning.propose",
            "suggestion",
            Some(&suggestion_id),
            None,
            Some(&baseline_digest),
            summary,
            &result,
        )
        .await?;
        tx.commit().await?;
        Ok(out)
    }

    /// Create a planning draft candidate (`planning.edit_draft`).
    pub async fn create_planning_candidate(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        create: &DraftCandidateCreate,
    ) -> PgResult<Value> {
        self.create_planning_candidate_bound(tenant_id, project_id, bearer, create, None)
            .await
    }

    pub async fn create_planning_candidate_bound(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        create: &DraftCandidateCreate,
        bind: Option<&PlanningCommandBind>,
    ) -> PgResult<Value> {
        let mut client = self.connect().await?;
        crate::check_schema(&client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let mut auth = authenticate_writer(&tx, tenant_id, project_id, bearer).await?;
        crate::delegation_auth::authorize_project_action(
            &tx,
            &mut auth,
            project_id,
            awr_team::Action::PlanningEditDraft,
        )
        .await?;
        let (baseline_digest, baseline_epoch) =
            current_baseline(&tx, tenant_id, project_id).await?;
        let known = known_work(&tx, tenant_id, project_id).await?;
        let policy = create
            .self_approve_policy
            .clone()
            .unwrap_or_else(OrdinaryPlanningSelfApprovePolicy::ordinary_default);
        ensure_independent_review_not_downgraded(
            "independent_review",
            &policy.delivery_completion_policy,
        )
        .map_err(map_team)?;
        let person = attested_actor_person(&auth.actor_id, create.author_person_id.as_deref())
            .map_err(map_team)?;
        let candidate_id = create
            .predetermined_candidate_id
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(new_id);
        // Resume: if the reserved candidate already exists, return its digest.
        if let Some(row) = tx
            .query_opt(
                "SELECT id, draft_revision, candidate_digest, state
                 FROM awr_team.planning_candidates
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant_id, &project_id, &candidate_id],
            )
            .await?
        {
            let result = json!({
                "candidate_id": row.get::<_, String>(0),
                "draft_revision": row.get::<_, i32>(1),
                "candidate_digest": row.get::<_, String>(2),
                "state": row.get::<_, String>(3),
                "resumed": true,
            });
            tx.commit().await?;
            return Ok(result);
        }
        let candidate = PlanningCandidate {
            codec: planning_codec_for_changes(&create.changes).into(),
            candidate_id: candidate_id.clone(),
            project_id: project_id.into(),
            author_person_id: person,
            author_actor_id: auth.actor_id.clone(),
            baseline_digest: baseline_digest.clone(),
            baseline_epoch: baseline_epoch.clone(),
            draft_revision: 1,
            changes: create.changes.clone(),
            suggestion_ids: create.suggestion_ids.clone(),
            state: CandidateState::Drafting,
            approval: None,
            allowed_spec_roots: create.allowed_spec_roots.clone(),
            project_goal_keys: if create.project_goal_keys.is_empty() {
                vec!["delivery".into()]
            } else {
                create.project_goal_keys.clone()
            },
        };
        let base_view = BaselineView {
            digest: &baseline_digest,
            epoch: &baseline_epoch,
            current: true,
            known_work_ids: known.ids.iter().map(String::as_str).collect(),
            known_external_keys: known.keys.iter().map(String::as_str).collect(),
        };
        validate_candidate(&candidate, &base_view).map_err(map_team)?;
        authorize_candidate_writable_scope(&tx, &auth, tenant_id, project_id, &candidate).await?;
        let digest = candidate.candidate_digest().map_err(map_team)?;
        let changes_json = serde_json::to_value(&candidate.changes)
            .map_err(|e| PgError::Protocol(e.to_string()))?;
        let suggestion_ids = serde_json::to_value(&candidate.suggestion_ids)
            .map_err(|e| PgError::Protocol(e.to_string()))?;
        let roots = serde_json::to_value(&candidate.allowed_spec_roots)
            .map_err(|e| PgError::Protocol(e.to_string()))?;
        let goals = serde_json::to_value(&candidate.project_goal_keys)
            .map_err(|e| PgError::Protocol(e.to_string()))?;
        let policy_json =
            serde_json::to_value(&policy).map_err(|e| PgError::Protocol(e.to_string()))?;
        tx.execute(
            "INSERT INTO awr_team.planning_candidates(
                tenant_id, project_id, id, author_person_id, author_actor_id, author_client_id,
                baseline_digest, baseline_epoch, draft_revision, candidate_digest, state,
                changes_json, suggestion_ids, allowed_spec_roots, project_goal_keys,
                self_approve_policy_json, delivery_completion_policy)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'drafting',$11,$12,$13,$14,$15,$16)",
            &[
                &tenant_id,
                &project_id,
                &candidate_id,
                &candidate.author_person_id,
                &auth.actor_id,
                &auth.client_id,
                &baseline_digest,
                &baseline_epoch,
                &(candidate.draft_revision as i32),
                &digest,
                &changes_json,
                &suggestion_ids,
                &roots,
                &goals,
                &policy_json,
                &policy.delivery_completion_policy,
            ],
        )
        .await?;
        append_history(
            &tx,
            tenant_id,
            project_id,
            &candidate_id,
            candidate.draft_revision as i32,
            &digest,
            &changes_json,
            &auth.actor_id,
            &auth.client_id,
        )
        .await?;
        for sid in &candidate.suggestion_ids {
            tx.execute(
                "UPDATE awr_team.planning_suggestions SET state='accepted_into_draft'
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND state='open'",
                &[&tenant_id, &project_id, sid],
            )
            .await?;
        }
        let impacts = load_impacts(&tx, tenant_id, project_id, &candidate).await?;
        let diff = build_candidate_diff(&candidate, impacts).map_err(map_team)?;
        let result = json!({
            "candidate_id": candidate_id,
            "draft_revision": candidate.draft_revision,
            "candidate_digest": digest,
            "state": "drafting",
            "diff": diff,
            "hard_delete_history_allowed": false,
            "forge_completion_via_status_allowed": false
        });
        let summary = json!({"candidate_id": candidate_id, "draft_revision": candidate.draft_revision, "candidate_digest": digest});
        let out = bind_planning_receipt_and_ops_audit(
            &tx,
            tenant_id,
            project_id,
            &auth,
            bind,
            "planning.edit_draft",
            "candidate",
            Some(&candidate_id),
            Some(&candidate_id),
            Some(&digest),
            summary,
            &result,
        )
        .await?;
        tx.commit().await?;
        Ok(out)
    }

    /// Edit an existing draft candidate. Clears prior approval and bumps revision.
    pub async fn edit_planning_candidate(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        candidate_id: &str,
        changes: Vec<DraftChange>,
    ) -> PgResult<Value> {
        let mut client = self.connect().await?;
        crate::check_schema(&client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let mut auth = authenticate_writer(&tx, tenant_id, project_id, bearer).await?;
        crate::delegation_auth::authorize_project_action(
            &tx,
            &mut auth,
            project_id,
            awr_team::Action::PlanningEditDraft,
        )
        .await?;
        lock_candidate(&tx, tenant_id, project_id, candidate_id).await?;
        let mut candidate = load_candidate(&tx, tenant_id, project_id, candidate_id).await?;
        let (baseline_digest, baseline_epoch) =
            current_baseline(&tx, tenant_id, project_id).await?;
        // Do not silently replace the stored baseline; require an explicit rebase
        // (fresh candidate) when the authority source has moved on.
        ensure_baseline_current(&candidate, &baseline_digest, &baseline_epoch)?;
        candidate = edit_candidate(candidate, changes).map_err(map_team)?;
        let known = known_work(&tx, tenant_id, project_id).await?;
        let base_view = BaselineView {
            digest: &baseline_digest,
            epoch: &baseline_epoch,
            current: true,
            known_work_ids: known.ids.iter().map(String::as_str).collect(),
            known_external_keys: known.keys.iter().map(String::as_str).collect(),
        };
        validate_candidate(&candidate, &base_view).map_err(map_team)?;
        authorize_candidate_writable_scope(&tx, &auth, tenant_id, project_id, &candidate).await?;
        let digest = candidate.candidate_digest().map_err(map_team)?;
        let changes_json = serde_json::to_value(&candidate.changes)
            .map_err(|e| PgError::Protocol(e.to_string()))?;
        tx.execute(
            "UPDATE awr_team.planning_candidates SET
                draft_revision=$4, candidate_digest=$5, state='drafting',
                changes_json=$6, baseline_digest=$7, baseline_epoch=$8,
                updated_at=clock_timestamp()
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[
                &tenant_id,
                &project_id,
                &candidate_id,
                &(candidate.draft_revision as i32),
                &digest,
                &changes_json,
                &baseline_digest,
                &baseline_epoch,
            ],
        )
        .await?;
        append_history(
            &tx,
            tenant_id,
            project_id,
            candidate_id,
            candidate.draft_revision as i32,
            &digest,
            &changes_json,
            &auth.actor_id,
            &auth.client_id,
        )
        .await?;
        let impacts = load_impacts(&tx, tenant_id, project_id, &candidate).await?;
        let diff = build_candidate_diff(&candidate, impacts).map_err(map_team)?;
        let result = json!({
            "candidate_id": candidate_id,
            "draft_revision": candidate.draft_revision,
            "candidate_digest": digest,
            "state": "drafting",
            "prior_approval_cleared": true,
            "diff": diff
        });
        {
            let summary = json!({"candidate_id": candidate_id, "draft_revision": candidate.draft_revision, "candidate_digest": digest});
            let mut audit = crate::ops_audit::write_from_auth(
                &auth,
                crate::ops_audit::OpsCategory::Planning,
                "planning.edit_draft",
                "candidate",
            );
            audit.target_id = Some(candidate_id.to_string());
            audit.change_id = Some(candidate_id.to_string());
            audit.source_version = Some(digest.clone());
            audit.digest = Some(crate::ops_audit::digest_of(&summary));
            audit.summary = summary;
            crate::ops_audit::record_in_tx(&tx, tenant_id, project_id, &audit).await?;
        }
        tx.commit().await?;
        Ok(result)
    }

    /// Preview exact diffs, affected tasks, and review requirements.
    pub async fn preview_planning_candidate(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        candidate_id: &str,
    ) -> PgResult<Value> {
        let mut client = self.connect().await?;
        crate::check_schema(&client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let mut auth = authenticate(&tx, tenant_id, project_id, bearer).await?;
        crate::delegation_auth::authorize_project_action(
            &tx,
            &mut auth,
            project_id,
            awr_team::Action::WorkRead,
        )
        .await?;
        let candidate = load_candidate(&tx, tenant_id, project_id, candidate_id).await?;
        // Membership WorkRead is not enough: confine contents to client readable scope.
        authorize_candidate_readable_scope(&tx, &auth, tenant_id, project_id, &candidate).await?;
        let impacts = load_impacts(&tx, tenant_id, project_id, &candidate).await?;
        let diff = build_candidate_diff(&candidate, impacts).map_err(map_team)?;
        let result = json!({
            "candidate_id": candidate_id,
            "state": match candidate.state {
                CandidateState::Drafting => "drafting",
                CandidateState::Approved => "approved",
                CandidateState::Published => "published",
                CandidateState::Superseded => "superseded",
            },
            "draft_revision": candidate.draft_revision,
            "diff": diff,
            "approval": candidate.approval,
        });
        tx.commit().await?;
        Ok(result)
    }

    /// Approve bound to the current candidate digest (`planning.approve`).
    pub async fn approve_planning_candidate(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        candidate_id: &str,
        candidate_digest: &str,
        author_person_id: Option<&str>,
    ) -> PgResult<Value> {
        let mut client = self.connect().await?;
        crate::check_schema(&client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let mut auth = authenticate_writer(&tx, tenant_id, project_id, bearer).await?;
        crate::delegation_auth::authorize_project_action(
            &tx,
            &mut auth,
            project_id,
            awr_team::Action::PlanningApprove,
        )
        .await?;
        let row = tx
            .query_one(
                "SELECT self_approve_policy_json, delivery_completion_policy
                 FROM awr_team.planning_candidates
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3 FOR UPDATE",
                &[&tenant_id, &project_id, &candidate_id],
            )
            .await
            .map_err(|_| PgError::Protocol("planning candidate not found".into()))?;
        let policy_json: Value = row.get(0);
        let delivery_policy: String = row.get(1);
        let policy: OrdinaryPlanningSelfApprovePolicy =
            serde_json::from_value(policy_json).map_err(|e| PgError::Protocol(e.to_string()))?;
        let mut candidate = load_candidate(&tx, tenant_id, project_id, candidate_id).await?;
        let (live_digest, live_epoch) = current_baseline(&tx, tenant_id, project_id).await?;
        ensure_baseline_current(&candidate, &live_digest, &live_epoch)?;
        authorize_candidate_readable_scope(&tx, &auth, tenant_id, project_id, &candidate).await?;
        // Do not copy a caller-supplied person into the approver scope. That
        // substitution both forges the receipt and clears the self-approve ban.
        attested_actor_person(&auth.actor_id, author_person_id).map_err(map_team)?;
        let scope = crate::workstream_auth::authority_scope(&auth, None, None);
        let resource = ResourceRef {
            tenant_id: tenant_id.into(),
            project_id: project_id.into(),
            workstream_id: None,
            work_id: None,
        };
        let self_approved = authorize_planning_approve(
            &scope,
            &resource,
            now_unix_ms(),
            &candidate,
            candidate_digest,
            &policy,
            &delivery_policy,
        )
        .map_err(map_team)?;
        let approval_id = new_id();
        tx.execute(
            "INSERT INTO awr_team.planning_approvals(
                tenant_id, project_id, id, candidate_id, candidate_digest, draft_revision,
                approver_person_id, approver_actor_id, approver_client_id, self_approved)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
            &[
                &tenant_id,
                &project_id,
                &approval_id,
                &candidate_id,
                &candidate_digest,
                &(candidate.draft_revision as i32),
                &scope.person_id,
                &auth.actor_id,
                &auth.client_id,
                &self_approved,
            ],
        )
        .await?;
        tx.execute(
            "UPDATE awr_team.planning_candidates SET state='approved', updated_at=clock_timestamp()
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant_id, &project_id, &candidate_id],
        )
        .await?;
        candidate.approval = Some(PlanningApproval {
            approval_id: approval_id.clone(),
            candidate_digest: candidate_digest.into(),
            approver_person_id: scope.person_id.clone(),
            approver_actor_id: auth.actor_id.clone(),
            self_approved,
        });
        candidate.state = CandidateState::Approved;
        let result = json!({
            "approval_id": approval_id,
            "candidate_id": candidate_id,
            "candidate_digest": candidate_digest,
            "draft_revision": candidate.draft_revision,
            "self_approved": self_approved,
            "state": "approved",
            "delivery_completion_policy": delivery_policy,
            "independent_review_downgraded": false
        });
        {
            let summary = json!({"approval_id": approval_id, "candidate_id": candidate_id, "candidate_digest": candidate_digest, "self_approved": self_approved});
            let mut audit = crate::ops_audit::write_from_auth(
                &auth,
                crate::ops_audit::OpsCategory::Planning,
                "planning.approve",
                "candidate",
            );
            audit.target_id = Some(candidate_id.to_string());
            audit.change_id = Some(candidate_id.to_string());
            audit.source_version = Some(candidate_digest.to_string());
            audit.digest = Some(crate::ops_audit::digest_of(&summary));
            audit.summary = summary;
            crate::ops_audit::record_in_tx(&tx, tenant_id, project_id, &audit).await?;
        }
        tx.commit().await?;
        Ok(result)
    }

    /// Publish is separate from approve and also digest-bound. Does not write
    /// authoritative source bytes (TMCP-022); records a publish receipt.
    pub async fn publish_planning_candidate(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        candidate_id: &str,
        candidate_digest: &str,
    ) -> PgResult<Value> {
        let mut client = self.connect().await?;
        crate::check_schema(&client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let mut auth = authenticate_writer(&tx, tenant_id, project_id, bearer).await?;
        crate::delegation_auth::authorize_project_action(
            &tx,
            &mut auth,
            project_id,
            awr_team::Action::PlanningPublish,
        )
        .await?;
        lock_candidate(&tx, tenant_id, project_id, candidate_id).await?;
        let mut candidate = load_candidate(&tx, tenant_id, project_id, candidate_id).await?;
        let (live_digest, live_epoch) = current_baseline(&tx, tenant_id, project_id).await?;
        ensure_baseline_current(&candidate, &live_digest, &live_epoch)?;
        authorize_candidate_readable_scope(&tx, &auth, tenant_id, project_id, &candidate).await?;
        if candidate.state != CandidateState::Approved {
            return Err(PgError::CandidateNotApproved);
        }
        let current_digest = candidate.candidate_digest().map_err(map_team)?;
        // Only an approval of this digest counts. Do not promote drafting or
        // published rows back to approved just because an older approval exists.
        if let Some(row) = tx
            .query_opt(
                "SELECT id, candidate_digest, approver_person_id, approver_actor_id, self_approved
                 FROM awr_team.planning_approvals
                 WHERE tenant_id=$1 AND project_id=$2 AND candidate_id=$3 AND candidate_digest=$4
                 ORDER BY decided_at DESC LIMIT 1",
                &[&tenant_id, &project_id, &candidate_id, &current_digest],
            )
            .await?
        {
            let approval_id: String = row.get(0);
            let digest: String = row.get(1);
            let person: String = row.get(2);
            let actor: String = row.get(3);
            let self_approved: bool = row.get(4);
            candidate.approval = Some(PlanningApproval {
                approval_id: approval_id.clone(),
                candidate_digest: digest,
                approver_person_id: person,
                approver_actor_id: actor,
                self_approved,
            });
            let scope = crate::workstream_auth::authority_scope(&auth, None, None);
            let resource = ResourceRef {
                tenant_id: tenant_id.into(),
                project_id: project_id.into(),
                workstream_id: None,
                work_id: None,
            };
            authorize_planning_publish(
                &scope,
                &resource,
                now_unix_ms(),
                &candidate,
                candidate_digest,
            )
            .map_err(map_team)?;
            let receipt_id = new_id();
            tx.execute(
                "INSERT INTO awr_team.planning_publish_receipts(
                    tenant_id, project_id, id, candidate_id, candidate_digest, draft_revision,
                    approval_id, publisher_actor_id, publisher_client_id, source_writeback_pending)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,true)",
                &[
                    &tenant_id,
                    &project_id,
                    &receipt_id,
                    &candidate_id,
                    &candidate_digest,
                    &(candidate.draft_revision as i32),
                    &approval_id,
                    &auth.actor_id,
                    &auth.client_id,
                ],
            )
            .await?;
            tx.execute(
                "UPDATE awr_team.planning_candidates SET state='published', updated_at=clock_timestamp()
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant_id, &project_id, &candidate_id],
            )
            .await?;
            let result = json!({
                "receipt_id": receipt_id,
                "candidate_id": candidate_id,
                "candidate_digest": candidate_digest,
                "draft_revision": candidate.draft_revision,
                "approval_id": approval_id,
                "state": "published",
                "source_writeback_pending": true,
                "source_bytes_written": false
            });
            {
                let summary = json!({
                    "receipt_id": receipt_id,
                    "candidate_id": candidate_id,
                    "candidate_digest": candidate_digest,
                    "approval_id": approval_id,
                });
                let mut audit = crate::ops_audit::write_from_auth(
                    &auth,
                    crate::ops_audit::OpsCategory::Planning,
                    "planning.publish",
                    "candidate",
                );
                audit.target_id = Some(candidate_id.to_string());
                audit.change_id = Some(candidate_id.to_string());
                audit.source_version = Some(candidate_digest.to_string());
                audit.digest = Some(crate::ops_audit::digest_of(&summary));
                audit.summary = summary;
                crate::ops_audit::record_in_tx(&tx, tenant_id, project_id, &audit).await?;
            }
            tx.commit().await?;
            Ok(result)
        } else {
            Err(PgError::CandidateNotApproved)
        }
    }

    /// Hard-delete of planning history is refused.
    pub async fn delete_planning_history(
        &self,
        _tenant_id: &str,
        _project_id: &str,
        _bearer: &str,
        _candidate_id: &str,
    ) -> PgResult<()> {
        Err(PgError::Protocol(
            "hard-delete of planning history is forbidden".into(),
        ))
    }

    /// Capability probe for TMCP-021 planning surfaces.
    pub fn planning_capabilities() -> Value {
        json!({
            "codec": PLANNING_CODEC,
            "supported_candidate_codecs": [PLANNING_CODEC, PLANNING_CODEC_V2, PLANNING_CODEC_V3, PLANNING_CODEC_V4, PLANNING_CODEC_V5, PLANNING_CODEC_V6],
            "cross_workstream_policy": {
                "review_assurance": ["team_independent", "simulated_member_independent"],
                "version_policy": ["fixed_delivery", "current_contract"],
                "declaration_only": true,
                "adoption_available": false
            },
            "execution_settlement": {
                "mode": "independent_workspace_v1",
                "workspace_id": "opaque_identity",
                "omission": "retain_source",
                "removal_supported": false,
                "declaration_only": true
            },
            "actions": [
                "planning.propose",
                "planning.edit_draft",
                "planning.approve",
                "planning.publish"
            ],
            "suggestion_claimable": false,
            "suggestion_adds_formal_work": false,
            "hard_delete_history_allowed": false,
            "forge_completion_via_status_allowed": false,
            "approve_publish_separated": true,
            "approval_bound_to_candidate_digest": true,
            "source_writeback": "tmcp_022"
        })
    }
}

struct KnownWork {
    ids: Vec<String>,
    keys: Vec<String>,
}

fn ensure_baseline_current(
    candidate: &PlanningCandidate,
    live_digest: &str,
    live_epoch: &str,
) -> PgResult<()> {
    if candidate.baseline_digest != live_digest || candidate.baseline_epoch != live_epoch {
        return Err(PgError::PreconditionsChanged);
    }
    Ok(())
}

/// Fail closed unless every exposed affected task is inside the client's writable
/// workstream grants. Membership template planning rights alone are insufficient.
async fn authorize_candidate_writable_scope(
    tx: &Transaction<'_>,
    auth: &ReaderAuthority,
    tenant_id: &str,
    project_id: &str,
    candidate: &PlanningCandidate,
) -> PgResult<()> {
    if auth.access.grants.iter().all(|g| !g.write) {
        return Err(PgError::Forbidden);
    }
    let mut seen = std::collections::BTreeSet::new();
    for change in &candidate.changes {
        let work_id = change.after.work_id.as_str();
        if !seen.insert(work_id.to_owned()) {
            continue;
        }
        let row = tx
            .query_opt(
                "SELECT workstream_id FROM awr_team.workstream_ownership
                 WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3",
                &[&tenant_id, &project_id, &work_id],
            )
            .await?;
        match row {
            Some(r) => {
                let stream: Id = r
                    .get::<_, String>(0)
                    .parse()
                    .map_err(|_| PgError::Forbidden)?;
                auth.access
                    .authorize(&auth.catalog, stream, WorkstreamAction::Write)
                    .map_err(|_| PgError::Forbidden)?;
            }
            None => {
                // Unbound/new draft work: require at least one live write grant
                // (already checked) but do not invent stream authority.
                if !auth.access.grants.iter().any(|g| g.write) {
                    return Err(PgError::Forbidden);
                }
            }
        }
    }
    Ok(())
}

/// Fail closed unless every exposed affected task is inside the client's readable
/// workstream grants. Membership template WorkRead alone is insufficient.
async fn authorize_candidate_readable_scope(
    tx: &Transaction<'_>,
    auth: &ReaderAuthority,
    tenant_id: &str,
    project_id: &str,
    candidate: &PlanningCandidate,
) -> PgResult<()> {
    authorize_domain_action(auth, awr_team::Action::WorkRead, None, None)?;
    if auth.access.grants.iter().all(|g| !g.read) {
        return Err(PgError::Forbidden);
    }
    let mut seen = std::collections::BTreeSet::new();
    for change in &candidate.changes {
        let work_id = change.after.work_id.as_str();
        if !seen.insert(work_id.to_owned()) {
            continue;
        }
        let row = tx
            .query_opt(
                "SELECT workstream_id FROM awr_team.workstream_ownership
                 WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3",
                &[&tenant_id, &project_id, &work_id],
            )
            .await?;
        match row {
            Some(r) => {
                let stream: Id = r
                    .get::<_, String>(0)
                    .parse()
                    .map_err(|_| PgError::Forbidden)?;
                auth.access
                    .authorize(&auth.catalog, stream, WorkstreamAction::Read)
                    .map_err(|_| PgError::Forbidden)?;
                authorize_domain_action(
                    auth,
                    awr_team::Action::WorkRead,
                    Some(stream),
                    Some(work_id),
                )?;
            }
            None => {
                // Unbound/new draft work: require at least one live read grant
                // (already checked) but do not invent stream authority.
                if !auth.access.grants.iter().any(|g| g.read) {
                    return Err(PgError::Forbidden);
                }
            }
        }
    }
    Ok(())
}

async fn current_baseline(
    tx: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
) -> PgResult<(String, String)> {
    let row = tx
        .query_one(
            "SELECT COALESCE(active_snapshot_id, ''), authority_epoch::text
             FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
            &[&tenant_id, &project_id],
        )
        .await?;
    let snapshot: String = row.get(0);
    let epoch: String = row.get(1);
    if snapshot.is_empty() {
        // No activated source yet — use a stable empty baseline so first-round
        // planning drafts can still be authored against an empty graph.
        return Ok((format!("sha256:{}", sha256_hex(b"empty-baseline")), epoch));
    }
    let digest: String = tx
        .query_one(
            "SELECT manifest_digest FROM awr_team.source_snapshots
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant_id, &project_id, &snapshot],
        )
        .await?
        .get(0);
    Ok((digest, epoch))
}

async fn known_work(
    tx: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
) -> PgResult<KnownWork> {
    let rows = tx
        .query(
            "SELECT id, external_key FROM awr_team.work_items
             WHERE tenant_id=$1 AND project_id=$2",
            &[&tenant_id, &project_id],
        )
        .await?;
    let mut ids = Vec::new();
    let mut keys = Vec::new();
    for row in rows {
        ids.push(row.get::<_, String>(0));
        keys.push(row.get::<_, String>(1));
    }
    Ok(KnownWork { ids, keys })
}

async fn append_history(
    tx: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    candidate_id: &str,
    draft_revision: i32,
    digest: &str,
    changes_json: &Value,
    actor_id: &str,
    client_id: &str,
) -> PgResult<()> {
    tx.execute(
        "INSERT INTO awr_team.planning_candidate_history(
            tenant_id, project_id, candidate_id, draft_revision, candidate_digest,
            changes_json, actor_id, client_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
        &[
            &tenant_id,
            &project_id,
            &candidate_id,
            &draft_revision,
            &digest,
            &changes_json,
            &actor_id,
            &client_id,
        ],
    )
    .await?;
    Ok(())
}

async fn lock_candidate(
    tx: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    candidate_id: &str,
) -> PgResult<()> {
    let row = tx
        .query_opt(
            "SELECT id FROM awr_team.planning_candidates
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3
             FOR UPDATE",
            &[&tenant_id, &project_id, &candidate_id],
        )
        .await?;
    if row.is_none() {
        return Err(PgError::Protocol("planning candidate not found".into()));
    }
    Ok(())
}

async fn load_candidate(
    tx: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    candidate_id: &str,
) -> PgResult<PlanningCandidate> {
    let row = tx
        .query_opt(
            "SELECT author_person_id, author_actor_id, baseline_digest, baseline_epoch,
                    draft_revision, candidate_digest, state, changes_json, suggestion_ids,
                    allowed_spec_roots, project_goal_keys
             FROM awr_team.planning_candidates
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant_id, &project_id, &candidate_id],
        )
        .await?
        .ok_or_else(|| PgError::Protocol("planning candidate not found".into()))?;
    let state_s: String = row.get(6);
    let state = match state_s.as_str() {
        "drafting" => CandidateState::Drafting,
        "approved" => CandidateState::Approved,
        "published" => CandidateState::Published,
        "superseded" => CandidateState::Superseded,
        other => {
            return Err(PgError::Protocol(format!(
                "unknown planning candidate state: {other}"
            )));
        }
    };
    let changes: Vec<DraftChange> =
        serde_json::from_value(row.get(7)).map_err(|e| PgError::Protocol(e.to_string()))?;
    let suggestion_ids = json_string_list(&row.get(8))?;
    let allowed_spec_roots = json_string_list(&row.get(9))?;
    let project_goal_keys = json_string_list(&row.get(10))?;
    let draft_revision: i32 = row.get(4);
    let mut candidate = PlanningCandidate {
        codec: planning_codec_for_changes(&changes).into(),
        candidate_id: candidate_id.into(),
        project_id: project_id.into(),
        author_person_id: row.get(0),
        author_actor_id: row.get(1),
        baseline_digest: row.get(2),
        baseline_epoch: row.get(3),
        draft_revision: draft_revision as u32,
        changes,
        suggestion_ids,
        state,
        approval: None,
        allowed_spec_roots,
        project_goal_keys,
    };
    // Verify stored digest still matches content (edited drafts bump revision).
    let computed = candidate.candidate_digest().map_err(map_team)?;
    let stored: String = row.get(5);
    if computed != stored && matches!(state, CandidateState::Drafting) {
        // Allow mismatch only if we're about to rewrite; for load used by
        // approve/publish the stored digest is authoritative for binding.
        candidate.baseline_digest = row.get(2);
    }
    if computed != stored && matches!(state, CandidateState::Approved | CandidateState::Published) {
        return Err(PgError::StaleApproval);
    }
    Ok(candidate)
}

async fn load_impacts(
    tx: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    candidate: &PlanningCandidate,
) -> PgResult<Vec<AffectedTaskImpact>> {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for change in &candidate.changes {
        let work_id = change.after.work_id.clone();
        if !seen.insert(work_id.clone()) {
            continue;
        }
        let runtime: Option<String> = tx
            .query_opt(
                "SELECT state FROM awr_team.work_runtime
                 WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3
                 LIMIT 1",
                &[&tenant_id, &project_id, &work_id],
            )
            .await?
            .map(|r| r.get(0));
        let claim: i64 = tx
            .query_one(
                "SELECT count(*) FROM awr_team.claims
                 WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='active'",
                &[&tenant_id, &project_id, &work_id],
            )
            .await?
            .get(0);
        let exec: Option<String> = tx
            .query_opt(
                "SELECT state FROM awr_team.executions
                 WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3
                   AND state NOT IN ('succeeded','failed','cancelled')
                 LIMIT 1",
                &[&tenant_id, &project_id, &work_id],
            )
            .await?
            .map(|r| r.get(0));
        out.push(AffectedTaskImpact {
            work_id: work_id.clone(),
            external_key: change.after.external_key.clone(),
            work_runtime_state: runtime.unwrap_or_else(|| "none".into()),
            execution_state: exec.unwrap_or_else(|| "none".into()),
            has_active_claim: claim > 0,
            review_requirement: change.after.completion_policy.clone(),
        });
    }
    Ok(out)
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
