use crate::error::{PgError, PgResult};
use crate::tx::{bind_scope, new_id};
use awr_team::delivery::LegacyPrSnapshot;
use awr_team::{CompletionView, EvidenceBundle, EvidenceGrade, ReviewPolicy, current_completion};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(crate) const AGENT_REVIEW_POLICY: &str = "caller_managed_execution_and_agent_review";
pub(crate) mod simulated_member;

pub(crate) fn approval_basis(independence_kind: &str) -> &'static str {
    match independence_kind {
        "team_independent" => "human_independent_review",
        "personal_self_review" => "human_author_self_review",
        "agent_review" => "agent_review",
        simulated_member::INDEPENDENCE => simulated_member::APPROVAL_BASIS,
        _ => "unspecified",
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct EvidenceRecord {
    pub id: String,
    pub work_id: String,
    pub trust_basis: String,
    pub digest: String,
    pub contract_hash: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReviewRound {
    pub id: String,
    pub work_id: String,
    pub round_index: i32,
    pub bundle_hash: String,
    pub state: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct CompletionReceipt {
    pub id: String,
    pub work_id: String,
    pub contract_hash: String,
    pub policy: String,
}

/// Versioned PR delivery binding (TMCP-031). GitHub facts are manually observed;
/// this is not webhook auto-sync.
#[derive(Clone, Debug, Serialize)]
pub struct PrDelivery {
    pub id: String,
    pub work_id: String,
    pub contract_hash: String,
    pub repository: String,
    pub pr_number: i32,
    pub pr_url: String,
    pub head_sha: String,
    pub merge_sha: Option<String>,
    pub test_evidence_id: Option<String>,
    pub gh_submitted: bool,
    pub gh_approved: bool,
    pub gh_merged: bool,
    pub fact_source: String,
    pub observed_at: String,
    pub state: String,
}

/// Shared mapping for the two legacy delivery reads. Both SELECTs use named
/// columns, so an additive projection cannot silently shift field positions.
pub(crate) fn legacy_pr_snapshot(row: &tokio_postgres::Row) -> LegacyPrSnapshot {
    LegacyPrSnapshot {
        delivery_id: row.get("id"),
        repository: row.get("repository"),
        pr_number: row.get("pr_number"),
        pr_url: row.get("pr_url"),
        head_sha: row.get("head_sha"),
        merge_sha: row.get("merge_sha"),
        submitted: row.get("gh_submitted"),
        approved: row.get("gh_approved"),
        merged: row.get("gh_merged"),
        fact_source: row.get("fact_source"),
        observed_at: row.get("observed_at"),
        contract_hash: row.get("contract_hash"),
        state: row.get("state"),
        test_evidence_id: row.get("test_evidence_id"),
    }
}

pub struct ReviewStore {
    pool: crate::PgPool,
}

impl ReviewStore {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            pool: crate::PgPool::new(url),
        }
    }

    /// Build from a validated `tokio_postgres::Config` (see PgPool::from_config).
    pub fn from_config(config: tokio_postgres::Config) -> Self {
        Self {
            pool: crate::PgPool::from_config(config),
        }
    }

    async fn connect(&self) -> PgResult<crate::PgClient> {
        self.pool.get().await
    }

    pub async fn record_evidence(
        &self,
        tenant_id: &str,
        project_id: &str,
        actor_id: &str,
        work_id: &str,
        contract_hash: &str,
        claimed_trust: Option<&str>,
        payload: &Value,
        artifact_bytes: Option<&[u8]>,
        input_digest: Option<&str>,
        dirty_tree: bool,
        execution_id: Option<&str>,
    ) -> PgResult<EvidenceRecord> {
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_scope(&tx, tenant_id, project_id).await?;
        lock_project(&tx, tenant_id, project_id).await?;
        let actor_kind: String = tx
            .query_opt(
                "SELECT kind FROM awr_team.actors WHERE tenant_id=$1 AND id=$2",
                &[&tenant_id, &actor_id],
            )
            .await?
            .map(|row| row.get(0))
            .ok_or(PgError::Forbidden)?;
        let trust_basis = assigned_trust(&actor_kind, claimed_trust);
        if dirty_tree && input_digest.is_none() && artifact_bytes.is_none() {
            return Err(PgError::EvidenceInvalid);
        }
        let payload = payload.clone();
        if payload.get("passed").and_then(Value::as_bool) == Some(true)
            && artifact_bytes.is_none()
            && payload.get("output_digest").is_none()
        {
            return Err(PgError::EvidenceInvalid);
        }
        // Two DIFFERENT digest contracts live on one evidence row
        // (CR #59 r3 P2-2):
        // - `output_digest` (artifact digest): sha256 of the submitted
        //   artifact bytes, verified against the persisted artifact.
        // - `execution_result_digest`: the executor-reported result digest
        //   the evidence declares to bind (payload "output_digest"),
        //   verified against the executions row at completion. The runner's
        //   result digest hashes path+content pairs, so it is NEVER equal
        //   to a single artifact's digest and must not be overwritten by it
        //   (CR #59 r3 P2-1).
        let output_digest = artifact_bytes.map(|bytes| sha256_hex(bytes));
        let execution_result_digest = payload
            .get("output_digest")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        // The evidence digest binds the FULL context — work, contract,
        // input, artifact identity and the declared execution result — so
        // an approval for one contract can never be reused by re-recording
        // the same payload under another contract (CR #42 P2-3).
        let digest = evidence_digest(
            work_id,
            contract_hash,
            input_digest,
            output_digest.as_deref(),
            execution_result_digest.as_deref(),
            &payload,
        )?;
        let mut artifact_id: Option<String> = None;
        if let Some(bytes) = artifact_bytes {
            let id = new_id();
            // Persist the actual bytes WITH the metadata: 'finalized' means
            // the content is durably readable, not merely described
            // (CR #42 P2-2).
            tx.execute(
                "INSERT INTO awr_team.artifacts(
                    tenant_id, project_id, id, object_key, sha256, byte_length,
                    media_type, state, created_by, content)
                 VALUES ($1,$2,$3,$4,$5,$6,'application/octet-stream','finalized',$7,$8)",
                &[
                    &tenant_id,
                    &project_id,
                    &id,
                    &format!("evidence/{id}"),
                    &sha256_hex(bytes),
                    &(bytes.len() as i64),
                    &actor_id,
                    &bytes,
                ],
            )
            .await?;
            artifact_id = Some(id);
        }
        let id = new_id();
        tx.execute(
            "INSERT INTO awr_team.evidence(
                tenant_id, project_id, id, work_id, execution_id, artifact_id,
                contract_hash, input_digest, output_digest, execution_result_digest,
                evidence_kind, trust_basis, digest, payload_json, created_by)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'report',$11,$12,$13,$14)",
            &[
                &tenant_id,
                &project_id,
                &id,
                &work_id,
                &execution_id.map(ToOwned::to_owned),
                &artifact_id,
                &contract_hash,
                &input_digest.map(ToOwned::to_owned),
                &output_digest,
                &execution_result_digest,
                &trust_basis,
                &digest,
                &payload,
                &actor_id,
            ],
        )
        .await?;
        crate::tx::emit_event(
            &tx,
            tenant_id,
            project_id,
            actor_id,
            work_id,
            "evidence.recorded",
            json!({"evidence_id": id, "trust_basis": trust_basis, "digest": digest}),
        )
        .await?;
        tx.commit().await?;
        Ok(EvidenceRecord {
            id,
            work_id: work_id.into(),
            trust_basis,
            digest,
            contract_hash: contract_hash.into(),
        })
    }

    pub async fn open_review(
        &self,
        tenant_id: &str,
        project_id: &str,
        author_actor_id: &str,
        work_id: &str,
        evidence_id: &str,
    ) -> PgResult<ReviewRound> {
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_scope(&tx, tenant_id, project_id).await?;
        lock_project(&tx, tenant_id, project_id).await?;
        let evidence = load_evidence(&tx, tenant_id, project_id, evidence_id).await?;
        if evidence.work_id != work_id {
            return Err(PgError::EvidenceInvalid);
        }
        // The author must be a real account in this tenant; accepting an
        // arbitrary string proves nothing about author/reviewer separation
        // (CR #42 P2-5).
        let author_exists: bool = tx
            .query_opt(
                "SELECT 1 FROM awr_team.actors WHERE tenant_id=$1 AND id=$2",
                &[&tenant_id, &author_actor_id],
            )
            .await?
            .is_some();
        if !author_exists {
            return Err(PgError::Forbidden);
        }
        invalidate_open_rounds(&tx, tenant_id, project_id, work_id, &evidence.digest).await?;
        let round_index: i32 = tx
            .query_one(
                "SELECT COALESCE(max(round_index),0)+1 FROM awr_team.review_rounds
                 WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3",
                &[&tenant_id, &project_id, &work_id],
            )
            .await?
            .get(0);
        let id = new_id();
        tx.execute(
            "INSERT INTO awr_team.review_rounds(
                tenant_id, project_id, id, work_id, round_index, bundle_hash,
                contract_hash, author_actor_id, state)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'open')",
            &[
                &tenant_id,
                &project_id,
                &id,
                &work_id,
                &round_index,
                &evidence.digest,
                &evidence.contract_hash,
                &author_actor_id,
            ],
        )
        .await?;
        crate::tx::emit_event(
            &tx,
            tenant_id,
            project_id,
            author_actor_id,
            work_id,
            "review.opened",
            json!({"round_id": id, "round_index": round_index, "bundle_hash": evidence.digest}),
        )
        .await?;
        tx.commit().await?;
        Ok(ReviewRound {
            id,
            work_id: work_id.into(),
            round_index,
            bundle_hash: evidence.digest,
            state: "open".into(),
        })
    }

    pub async fn decide_review(
        &self,
        tenant_id: &str,
        project_id: &str,
        reviewer_actor_id: &str,
        round_id: &str,
        decision: &str,
        reason: &str,
    ) -> PgResult<ReviewRound> {
        if !matches!(decision, "approve" | "reject") {
            return Err(PgError::Protocol("invalid review decision".into()));
        }
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_scope(&tx, tenant_id, project_id).await?;
        lock_project(&tx, tenant_id, project_id).await?;
        let row = tx
            .query_opt(
                "SELECT work_id, author_actor_id, bundle_hash, state, round_index, contract_hash
                 FROM awr_team.review_rounds
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3
                 FOR UPDATE",
                &[&tenant_id, &project_id, &round_id],
            )
            .await?
            .ok_or_else(|| PgError::Protocol("review round not found".into()))?;
        let work_id: String = row.get(0);
        let author: String = row.get(1);
        let bundle_hash: String = row.get(2);
        let state: String = row.get(3);
        let round_index: i32 = row.get(4);
        if state != "open" {
            return Err(PgError::ReviewRequired);
        }
        // Reviewer: real, active, approval-capable member (status +
        // membership role), and not an agent (CR #42 P2-5). Independence is
        // judged by responsible person, not by a second agent of the same human.
        let reviewer_kind: String = tx
            .query_opt(
                "SELECT kind FROM awr_team.actors WHERE tenant_id=$1 AND id=$2",
                &[&tenant_id, &reviewer_actor_id],
            )
            .await?
            .map(|r| r.get(0))
            .ok_or(PgError::Forbidden)?;
        if reviewer_kind == "agent" {
            return Err(PgError::AuthorCannotReview);
        }
        crate::tx::validate_reviewer(&tx, tenant_id, project_id, reviewer_actor_id).await?;
        crate::tx::require_independent_review_grant(&tx, tenant_id, project_id, reviewer_actor_id)
            .await?;
        // Unbound legacy agents compare by actor id; humans/agents with
        // bindings compare by responsible person (WS-018).
        let author_person = match resolve_person_id(&tx, tenant_id, project_id, &author).await {
            Ok(person) => person,
            Err(_) => author.clone(),
        };
        let reviewer_person =
            resolve_person_id(&tx, tenant_id, project_id, reviewer_actor_id).await?;
        let same_person = author_person == reviewer_person || reviewer_actor_id == author;
        let contract = current_contract(&tx, tenant_id, project_id, "main", &work_id).await?;
        let policy = contract
            .get("completion_policy")
            .and_then(Value::as_str)
            .unwrap_or("trusted_execution_and_review");
        if matches!(policy, AGENT_REVIEW_POLICY | simulated_member::POLICY) {
            return Err(PgError::Unsupported(
                "Agent review requires the authenticated workstream command path".into(),
            ));
        }
        let independence_kind = if same_person {
            if decision == "approve" && !self_review_permitted(policy) {
                return Err(PgError::AuthorCannotReview);
            }
            if decision == "approve" {
                "personal_self_review"
            } else {
                // Reject/return by the authoring person is still a personal action.
                "personal_self_review"
            }
        } else {
            "team_independent"
        };
        let next = if decision == "approve" {
            "approved"
        } else {
            "rejected"
        };
        tx.execute(
            "INSERT INTO awr_team.review_decisions(
                tenant_id, project_id, id, review_round_id, work_id, bundle_hash,
                reviewer_actor_id, decision, reason, reviewer_person_id, independence_kind,
                approval_basis)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
            &[
                &tenant_id,
                &project_id,
                &new_id(),
                &round_id,
                &work_id,
                &bundle_hash,
                &reviewer_actor_id,
                &decision,
                &reason,
                &reviewer_person,
                &independence_kind,
                &approval_basis(independence_kind),
            ],
        )
        .await?;
        tx.execute(
            "UPDATE awr_team.review_rounds SET state=$4
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant_id, &project_id, &round_id, &next],
        )
        .await?;
        crate::tx::emit_event(
            &tx,
            tenant_id,
            project_id,
            reviewer_actor_id,
            &work_id,
            "review.decided",
            json!({
                "round_id": round_id,
                "decision": decision,
                "independence_kind": independence_kind,
                "approval_basis": approval_basis(independence_kind),
                "reviewer_person_id": reviewer_person,
                "author_person_id": author_person,
                "team_independent_acceptance": independence_kind == "team_independent"
                    && decision == "approve",
            }),
        )
        .await?;
        {
            let summary = json!({
                "round_id": round_id,
                "decision": decision,
                "independence_kind": independence_kind,
                "reviewer_person_id": reviewer_person,
            });
            let audit = crate::ops_audit::OpsAuditWrite {
                category: crate::ops_audit::OpsCategory::Delivery,
                action: "review.decide".into(),
                result: "committed",
                person_id: Some(reviewer_person.clone()),
                actor_id: reviewer_actor_id.to_string(),
                client_id: "review".into(),
                target_kind: "review".into(),
                target_id: Some(round_id.to_string()),
                work_id: Some(work_id.clone()),
                change_id: None,
                request_id: None,
                membership_version: None,
                authority_version: None,
                policy_version: Some(awr_team::PERMISSION_POLICY_VERSION as i32),
                source_version: None,
                digest: Some(crate::ops_audit::digest_of(&summary)),
                summary,
            };
            crate::ops_audit::record_in_tx(&tx, tenant_id, project_id, &audit).await?;
        }
        tx.commit().await?;
        Ok(ReviewRound {
            id: round_id.into(),
            work_id,
            round_index,
            bundle_hash,
            state: next.into(),
        })
    }

    /// Complete a work. Idempotent on (client_id, request_id): the same
    /// request returns the original receipt, changed parameters conflict
    /// (CR #42 P2-9). All gates, the receipt, the dependency mapping and the
    /// runtime update commit in ONE transaction (CR #42 P2-10).
    pub async fn complete(
        &self,
        tenant_id: &str,
        project_id: &str,
        actor_id: &str,
        client_id: &str,
        request_id: &str,
        work_id: &str,
        scope_id: &str,
        evidence_id: &str,
        requested_policy: Option<&str>,
        context_complete: bool,
    ) -> PgResult<CompletionReceipt> {
        let op_args = json!({
            "work_id": work_id,
            "scope_id": scope_id,
            "evidence_id": evidence_id,
            "requested_policy": requested_policy,
            "context_complete": context_complete,
        });
        let request_hash = awr_team::request_hash(&json!({
            "op": "work.complete",
            "request_id": request_id,
            "args": op_args,
        }))
        .map_err(|e| PgError::Protocol(e.to_string()))?;
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_scope(&tx, tenant_id, project_id).await?;
        lock_project(&tx, tenant_id, project_id).await?;
        if let Some((stored_hash, result)) =
            load_operation(&tx, tenant_id, project_id, actor_id, client_id, request_id).await?
        {
            if stored_hash != request_hash {
                return Err(PgError::IdempotencyConflict);
            }
            let receipt_id = result
                .get("receipt_id")
                .and_then(Value::as_str)
                .ok_or_else(|| PgError::Protocol("stored completion receipt missing id".into()))?;
            return Ok(CompletionReceipt {
                id: receipt_id.into(),
                work_id: work_id.into(),
                contract_hash: result
                    .get("contract_hash")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                policy: result
                    .get("policy")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
            });
        }
        if !context_complete {
            return Err(PgError::ContextIncomplete);
        }
        let unknown: i64 = tx
            .query_one(
                "SELECT count(*) FROM awr_team.executions
                 WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='unknown'",
                &[&tenant_id, &project_id, &work_id],
            )
            .await?
            .get(0);
        if unknown > 0 {
            return Err(PgError::RecoveryBlocked);
        }
        // An explicit recovery block is a deliberate hold; completion must
        // not skip it nor silently clear it (CR #42 P2-8).
        let blocked: bool = tx
            .query_opt(
                "SELECT recovery_blocked FROM awr_team.work_runtime
                 WHERE tenant_id=$1 AND project_id=$2 AND scope_id=$3 AND work_id=$4",
                &[&tenant_id, &project_id, &scope_id, &work_id],
            )
            .await?
            .map(|row| row.get(0))
            .unwrap_or(false);
        if blocked {
            return Err(PgError::RecoveryBlocked);
        }
        let contract = current_contract(&tx, tenant_id, project_id, scope_id, work_id).await?;
        let policy = contract
            .get("completion_policy")
            .and_then(Value::as_str)
            .unwrap_or("trusted_execution_and_review");
        if let Some(requested) = requested_policy {
            if requested != policy {
                return Err(PgError::PolicyDowngrade);
            }
        }
        if matches!(policy, AGENT_REVIEW_POLICY | simulated_member::POLICY) {
            return Err(PgError::Unsupported(
                "Agent-reviewed completion requires the authenticated workstream command path"
                    .into(),
            ));
        }
        let evidence = load_evidence(&tx, tenant_id, project_id, evidence_id).await?;
        if evidence.work_id != work_id || evidence.contract_hash != current_contract_hash(&contract)
        {
            return Err(PgError::EvidenceInvalid);
        }
        if evidence_bytes_changed(&evidence)? {
            return Err(PgError::EvidenceInvalid);
        }
        if evidence.artifact_id.is_some() {
            let row = tx
                .query_opt(
                    "SELECT sha256, state, content FROM awr_team.artifacts
                     WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                    &[&tenant_id, &project_id, &evidence.artifact_id],
                )
                .await?
                .ok_or(PgError::EvidenceInvalid)?;
            let sha: String = row.get(0);
            let state: String = row.get(1);
            // The artifact must be ACTUALLY readable: re-read the persisted
            // bytes and verify them against both the artifact digest and the
            // evidence output digest (CR #42 P2-2). Metadata-only legacy
            // records cannot satisfy completion.
            let content: Option<Vec<u8>> = row.get(2);
            let content = content.ok_or(PgError::EvidenceInvalid)?;
            if state != "finalized"
                || sha256_hex(&content) != sha
                || evidence.output_digest.as_deref() != Some(sha.as_str())
            {
                return Err(PgError::EvidenceInvalid);
            }
        }
        let grade = match evidence.trust_basis.as_str() {
            "trusted_executor" => EvidenceGrade::TrustedExecutionReceipt,
            "human_review" => EvidenceGrade::AuthorizedReview,
            _ => EvidenceGrade::AgentSelfReport,
        };
        if policy == "ordinary_confirm" {
            let kind: String = tx
                .query_one(
                    "SELECT kind FROM awr_team.actors WHERE tenant_id=$1 AND id=$2",
                    &[&tenant_id, &actor_id],
                )
                .await?
                .get(0);
            if kind != "human" {
                return Err(PgError::Forbidden);
            }
        } else if grade != EvidenceGrade::TrustedExecutionReceipt {
            // trusted_execution_and_review requires a bound trusted
            // receipt — an AuthorizedReview grade (human material) must NOT
            // satisfy it, and AgentSelfReport is never trusted (CR #59 P2-1).
            return Err(PgError::EvidenceInvalid);
        } else {
            // A trusted-execution grade must bind to a SUCCEEDED execution
            // of the same contract and input by the delegated executor —
            // actor kind alone proves nothing (CR #42 P2-1). A report that
            // admits failure (passed=false) can never complete.
            if evidence.payload.get("passed").and_then(Value::as_bool) == Some(false) {
                return Err(PgError::EvidenceInvalid);
            }
            let execution_id = evidence
                .execution_id
                .as_deref()
                .ok_or(PgError::EvidenceInvalid)?;
            let exec = tx
                .query_opt(
                    "SELECT state, contract_hash, input_digest, executor_actor_id,
                            scope_id, result_digest
                     FROM awr_team.executions
                     WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                    &[&tenant_id, &project_id, &execution_id],
                )
                .await?
                .ok_or(PgError::EvidenceInvalid)?;
            let exec_state: String = exec.get(0);
            let exec_contract: String = exec.get(1);
            let exec_input: Option<String> = exec.get(2);
            let exec_executor: String = exec.get(3);
            let exec_scope: String = exec.get(4);
            let exec_result: Option<String> = exec.get(5);
            if exec_state != "succeeded" || exec_contract != evidence.contract_hash {
                return Err(PgError::EvidenceInvalid);
            }
            // The binding is complete: the evidence's submitter IS the
            // delegated executor, the scope matches the completion scope,
            // input digests are EQUAL on both sides (NULL does not skip),
            // and the declared execution result digest is present and equal
            // on both sides (CR #59 P2-1, CR #59 r3 P2-1).
            if exec_executor != evidence.created_by || exec_scope != scope_id {
                return Err(PgError::EvidenceInvalid);
            }
            if exec_input != evidence.input_digest {
                return Err(PgError::EvidenceInvalid);
            }
            // Output binding under the strict policy: the execution must
            // carry a recorded result digest AND the evidence must declare
            // the SAME digest. A missing value on either side proves
            // nothing and fails closed — None == None is not evidence of
            // output identity (CR #59 r3 P2-1). This binds the EXECUTION
            // RESULT digest; the artifact digest was verified against the
            // persisted artifact bytes above and is a separate contract
            // (CR #59 r3 P2-2).
            let declared_result = evidence
                .execution_result_digest
                .as_deref()
                .ok_or(PgError::EvidenceInvalid)?;
            let recorded_result = exec_result.as_deref().ok_or(PgError::EvidenceInvalid)?;
            if declared_result != recorded_result {
                return Err(PgError::EvidenceInvalid);
            }
        }
        let (binding_valid, dependency_links) =
            required_dependencies_covered(&tx, tenant_id, project_id, work_id, scope_id, &contract)
                .await?;
        let pinned_round: Option<String> = tx
            .query_opt(
                "SELECT id FROM awr_team.review_rounds
                 WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND bundle_hash=$4
                 ORDER BY round_index DESC LIMIT 1",
                &[&tenant_id, &project_id, &work_id, &evidence.digest],
            )
            .await?
            .map(|row| row.get(0));
        let mut review =
            current_review(&tx, tenant_id, project_id, work_id, &evidence.digest).await?;
        if policy == "ordinary_confirm" {
            review.required = false;
        }
        // The consumed review must be for THIS contract's bundle — a new
        // contract never reuses an old approval (CR #42 P2-3). Only applies
        // where a review is required at all (ordinary_confirm has none).
        if policy != "ordinary_confirm" {
            let round_contract: Option<String> = tx
                .query_opt(
                    "SELECT contract_hash FROM awr_team.review_rounds
                     WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND bundle_hash=$4
                     ORDER BY round_index DESC LIMIT 1",
                    &[&tenant_id, &project_id, &work_id, &evidence.digest],
                )
                .await?
                .map(|row| row.get(0));
            if round_contract.as_deref() != Some(evidence.contract_hash.as_str()) {
                return Err(PgError::ReviewRequired);
            }
        }
        let bundle = EvidenceBundle {
            grade,
            contract_hash: evidence.contract_hash.clone(),
            artifact_digest: evidence.output_digest.clone(),
            accessible: true,
        };
        let view = current_completion(
            false,
            true,
            Some(&evidence.contract_hash),
            &evidence.contract_hash,
            binding_valid,
            Some(&bundle),
            &review,
        );
        if view != CompletionView::CurrentlyVerified {
            return Err(PgError::CompletionRejected);
        }
        // The receipt records the ACTUAL approver (from the pinned round's
        // decision) separately from the submitter (CR #42 audit note).
        // The approver comes from the SAME pinned round the gate consumed;
        // never re-pick from other rounds at receipt time (CR #59 P2-3).
        let approver: Option<String> = match &pinned_round {
            Some(round) => tx
                .query_opt(
                    "SELECT reviewer_actor_id FROM awr_team.review_decisions
                     WHERE tenant_id=$1 AND project_id=$2 AND review_round_id=$3
                       AND decision='approve'
                     ORDER BY created_at DESC LIMIT 1",
                    &[&tenant_id, &project_id, round],
                )
                .await?
                .map(|row| row.get(0)),
            None => None,
        };

        // Active PR delivery (if any) must still match live contract/head binding.
        // Merged/approved GitHub state never substitutes for AWR acceptance.
        let pr_row = tx
            .query_opt(
                "SELECT id, contract_hash, head_sha, gh_merged, state, author_actor_id,
                        owner_person_id, executor_actor_id
                 FROM awr_team.pr_deliveries
                 WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='active'
                 ORDER BY created_at DESC LIMIT 1",
                &[&tenant_id, &project_id, &work_id],
            )
            .await?;
        let mut pr_delivery_id: Option<String> = None;
        let mut pr_author: Option<String> = None;
        let mut pr_owner: Option<String> = None;
        let mut pr_executor: Option<String> = None;
        if let Some(pr) = &pr_row {
            let pr_contract: String = pr.get(1);
            if pr_contract != evidence.contract_hash {
                return Err(PgError::PreconditionsChanged);
            }
            pr_delivery_id = Some(pr.get(0));
            pr_author = pr.get(5);
            pr_owner = pr.get(6);
            pr_executor = pr.get(7);
            let _gh_merged: bool = pr.get(3);
            // Explicit: merge flag is recorded but does not authorize completion.
            let _ = _gh_merged;
        }
        let verified_executor = if let Some(eid) = evidence.execution_id.as_deref() {
            tx.query_opt(
                "SELECT executor_actor_id FROM awr_team.executions
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant_id, &project_id, &eid],
            )
            .await?
            .map(|r| r.get::<_, String>(0))
        } else {
            None
        };
        let author_actor = pr_author
            .clone()
            .or_else(|| Some(evidence.created_by.clone()));
        let executor_actor = pr_executor.clone().or(verified_executor);
        let approved_by = json!({
            "approved_by": approver,
            "submitted_by": actor_id,
            "author_actor_id": author_actor,
            "owner_person_id": pr_owner.clone(),
            "executor_actor_id": executor_actor,
            "reviewer_actor_id": approver.clone(),
            "final_submitter_actor_id": actor_id,
            "pr_delivery_id": pr_delivery_id.clone(),
            "github_merged_does_not_complete": true,
        });
        let dependency_binding_hash = sha256_hex(json!(dependency_links).to_string().as_bytes());
        let receipt_id = new_id();
        tx.execute(
            "INSERT INTO awr_team.completion_receipts(
                tenant_id, project_id, id, work_id, scope_id, contract_hash,
                result_digest, dependency_binding_hash, evidence_bundle_hash,
                policy, approved_by_json,
                author_actor_id, owner_person_id, executor_actor_id,
                final_submitter_actor_id, pr_delivery_id)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)",
            &[
                &tenant_id,
                &project_id,
                &receipt_id,
                &work_id,
                &scope_id,
                &evidence.contract_hash,
                &evidence.digest,
                &dependency_binding_hash,
                &evidence.digest,
                &policy,
                &approved_by,
                &author_actor,
                &pr_owner,
                &executor_actor,
                &actor_id,
                &pr_delivery_id,
            ],
        )
        .await?;
        // Real mappings for audit: which evidence and which predecessor
        // receipts this completion consumed (CR #42 audit note).
        tx.execute(
            "INSERT INTO awr_team.completion_evidence(
                tenant_id, project_id, completion_id, evidence_id, criterion_id)
             VALUES ($1,$2,$3,$4,'contract')",
            &[&tenant_id, &project_id, &receipt_id, &evidence_id],
        )
        .await?;
        for (upstream_work, upstream_receipt) in &dependency_links {
            tx.execute(
                "INSERT INTO awr_team.completion_dependencies(
                    tenant_id, project_id, completion_id, predecessor_work_id,
                    predecessor_completion_id)
                 VALUES ($1,$2,$3,$4,$5)",
                &[
                    &tenant_id,
                    &project_id,
                    &receipt_id,
                    &upstream_work,
                    &upstream_receipt,
                ],
            )
            .await?;
        }
        tx.execute(
            "INSERT INTO awr_team.work_runtime(
                tenant_id, project_id, scope_id, work_id, state, work_version, last_fence,
                selected_completion_id)
             VALUES ($1,$2,$3,$4,'completed',1,0,$5)
             ON CONFLICT (tenant_id, project_id, scope_id, work_id)
             DO UPDATE SET state='completed', selected_completion_id=$5,
                 work_version = awr_team.work_runtime.work_version + 1",
            &[&tenant_id, &project_id, &scope_id, &work_id, &receipt_id],
        )
        .await?;
        let committed_revision = crate::tx::emit_event(
            &tx,
            tenant_id,
            project_id,
            actor_id,
            work_id,
            "work.completed",
            json!({"receipt_id": receipt_id, "evidence_id": evidence_id, "policy": policy}),
        )
        .await?;
        let result = json!({
            "receipt_id": receipt_id,
            "contract_hash": evidence.contract_hash,
            "policy": policy,
            "committed_project_revision": committed_revision.to_string(),
        });
        store_operation(
            &tx,
            tenant_id,
            project_id,
            actor_id,
            client_id,
            request_id,
            "work.complete",
            &request_hash,
            committed_revision,
            &result,
        )
        .await?;
        {
            let summary = json!({
                "receipt_id": receipt_id,
                "contract_hash": evidence.contract_hash,
                "policy": policy,
                "request_id": request_id,
            });
            let audit = crate::ops_audit::OpsAuditWrite {
                category: crate::ops_audit::OpsCategory::Delivery,
                action: "delivery.finalize".into(),
                result: "committed",
                person_id: None,
                actor_id: actor_id.to_string(),
                client_id: client_id.to_string(),
                target_kind: "completion".into(),
                target_id: Some(receipt_id.clone()),
                work_id: Some(evidence.work_id.clone()),
                change_id: None,
                request_id: Some(request_id.to_string()),
                membership_version: None,
                authority_version: None,
                policy_version: Some(awr_team::PERMISSION_POLICY_VERSION as i32),
                source_version: Some(evidence.contract_hash.clone()),
                digest: Some(crate::ops_audit::digest_of(&summary)),
                summary,
            };
            crate::ops_audit::record_in_tx(&tx, tenant_id, project_id, &audit).await?;
        }
        tx.commit().await?;
        Ok(CompletionReceipt {
            id: receipt_id,
            work_id: work_id.into(),
            contract_hash: evidence.contract_hash,
            policy: policy.into(),
        })
    }

    /// Register a PR delivery binding after authorized human GitHub verification.
    /// Does not claim webhook auto-sync; requires fact_source + observed_at.
    pub async fn register_pr_delivery(
        &self,
        tenant_id: &str,
        project_id: &str,
        actor_id: &str,
        work_id: &str,
        contract_hash: &str,
        repository: &str,
        pr_number: i32,
        pr_url: &str,
        head_sha: &str,
        merge_sha: Option<&str>,
        test_evidence_id: Option<&str>,
        fact_source: &str,
        observed_at: &str,
        author_actor_id: Option<&str>,
        owner_person_id: Option<&str>,
        executor_actor_id: Option<&str>,
        gh_submitted: bool,
        gh_approved: bool,
        gh_merged: bool,
    ) -> PgResult<PrDelivery> {
        if pr_number <= 0
            || repository.trim().is_empty()
            || repository.len() > 256
            || pr_url.trim().is_empty()
            || pr_url.len() > 512
            || !is_git_sha(head_sha)
            || merge_sha.is_some_and(|s| !is_git_sha(s))
            || !matches!(
                fact_source,
                "authorized_human_github_verification" | "operator_recorded_observation"
            )
            || observed_at.trim().is_empty()
        {
            return Err(PgError::Protocol("invalid pr delivery registration".into()));
        }
        // URL alone is never sufficient proof — fact_source must be an authorized observation.
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_scope(&tx, tenant_id, project_id).await?;
        lock_project(&tx, tenant_id, project_id).await?;
        let live = current_contract(&tx, tenant_id, project_id, "main", work_id).await?;
        let live_hash = current_contract_hash(&live);
        if live_hash != contract_hash {
            return Err(PgError::PreconditionsChanged);
        }
        let mut test_digest: Option<String> = None;
        if let Some(eid) = test_evidence_id {
            let ev = load_evidence(&tx, tenant_id, project_id, eid).await?;
            if ev.work_id != work_id || ev.contract_hash != contract_hash {
                return Err(PgError::EvidenceInvalid);
            }
            test_digest = Some(ev.digest);
        }
        // Invalidate prior active deliveries for this work when head/contract diverge.
        tx.execute(
            "UPDATE awr_team.pr_deliveries
             SET state='invalidated', invalidation_reason='superseded_by_new_registration'
             WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='active'",
            &[&tenant_id, &project_id, &work_id],
        )
        .await?;
        let id = new_id();
        let observed_ts = validate_observed_at(observed_at)?;
        tx.execute(
            "INSERT INTO awr_team.pr_deliveries(
                tenant_id, project_id, id, work_id, contract_hash, repository, pr_number, pr_url,
                head_sha, merge_sha, test_evidence_id, test_evidence_digest,
                gh_submitted, gh_approved, gh_merged, fact_source, observed_at,
                registered_by_actor_id, author_actor_id, owner_person_id, executor_actor_id, state)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,'active')",
            &[
                &tenant_id,
                &project_id,
                &id,
                &work_id,
                &contract_hash,
                &repository,
                &pr_number,
                &pr_url,
                &head_sha,
                &merge_sha,
                &test_evidence_id,
                &test_digest,
                &gh_submitted,
                &gh_approved,
                &gh_merged,
                &fact_source,
                &observed_ts,
                &actor_id,
                &author_actor_id,
                &owner_person_id,
                &executor_actor_id,
            ],
        )
        .await?;
        // Head change invalidates open review rounds for mismatched bundle/contract.
        invalidate_open_rounds_for_contract(&tx, tenant_id, project_id, work_id, contract_hash)
            .await?;
        crate::tx::emit_event(
            &tx,
            tenant_id,
            project_id,
            actor_id,
            work_id,
            "delivery.pr_registered",
            json!({
                "delivery_id": id,
                "repository": repository,
                "pr_number": pr_number,
                "head_sha": head_sha,
                "merge_sha": merge_sha,
                "fact_source": fact_source,
                "observed_at": observed_at,
                "webhook_auto_sync": false,
            }),
        )
        .await?;
        {
            let summary = json!({
                "delivery_id": id,
                "repository": repository,
                "pr_number": pr_number,
                "head_sha": head_sha,
                "fact_source": fact_source,
                "contract_hash": contract_hash,
            });
            let audit = crate::ops_audit::OpsAuditWrite {
                category: crate::ops_audit::OpsCategory::Delivery,
                action: "delivery.register_pr".into(),
                result: "committed",
                person_id: owner_person_id.map(|s| s.to_string()),
                actor_id: actor_id.to_string(),
                client_id: "delivery".into(),
                target_kind: "delivery".into(),
                target_id: Some(id.clone()),
                work_id: Some(work_id.to_string()),
                change_id: None,
                request_id: None,
                membership_version: None,
                authority_version: None,
                policy_version: Some(awr_team::PERMISSION_POLICY_VERSION as i32),
                source_version: Some(contract_hash.to_string()),
                digest: Some(crate::ops_audit::digest_of(&summary)),
                summary,
            };
            crate::ops_audit::record_in_tx(&tx, tenant_id, project_id, &audit).await?;
        }
        tx.commit().await?;
        Ok(PrDelivery {
            id,
            work_id: work_id.into(),
            contract_hash: contract_hash.into(),
            repository: repository.into(),
            pr_number,
            pr_url: pr_url.into(),
            head_sha: head_sha.into(),
            merge_sha: merge_sha.map(str::to_owned),
            test_evidence_id: test_evidence_id.map(str::to_owned),
            gh_submitted,
            gh_approved,
            gh_merged,
            fact_source: fact_source.into(),
            observed_at: observed_at.into(),
            state: "active".into(),
        })
    }

    /// Update GitHub observation flags on an active PR delivery (still not webhook sync).
    pub async fn observe_pr_delivery(
        &self,
        tenant_id: &str,
        project_id: &str,
        actor_id: &str,
        delivery_id: &str,
        expected_head_sha: &str,
        fact_source: &str,
        observed_at: &str,
        gh_approved: Option<bool>,
        gh_merged: Option<bool>,
        merge_sha: Option<&str>,
    ) -> PgResult<PrDelivery> {
        if !is_git_sha(expected_head_sha)
            || !matches!(
                fact_source,
                "authorized_human_github_verification" | "operator_recorded_observation"
            )
            || merge_sha.is_some_and(|s| !is_git_sha(s))
        {
            return Err(PgError::Protocol("invalid pr observation".into()));
        }
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_scope(&tx, tenant_id, project_id).await?;
        lock_project(&tx, tenant_id, project_id).await?;
        let row = tx
            .query_opt(
                "SELECT work_id, contract_hash, repository, pr_number, pr_url, head_sha, merge_sha,
                        test_evidence_id, gh_submitted, gh_approved, gh_merged, state
                 FROM awr_team.pr_deliveries
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3 FOR UPDATE",
                &[&tenant_id, &project_id, &delivery_id],
            )
            .await?
            .ok_or_else(|| PgError::Protocol("pr delivery not found".into()))?;
        let work_id: String = row.get(0);
        let contract_hash: String = row.get(1);
        let repository: String = row.get(2);
        let pr_number: i32 = row.get(3);
        let pr_url: String = row.get(4);
        let head_sha: String = row.get(5);
        let mut cur_merge: Option<String> = row.get(6);
        let test_evidence_id: Option<String> = row.get(7);
        let gh_submitted: bool = row.get(8);
        let mut cur_approved: bool = row.get(9);
        let mut cur_merged: bool = row.get(10);
        let state: String = row.get(11);
        if state != "active" {
            return Err(PgError::PreconditionsChanged);
        }
        if head_sha != expected_head_sha {
            tx.execute(
                "UPDATE awr_team.pr_deliveries
                 SET state='invalidated', invalidation_reason='head_sha_mismatch'
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant_id, &project_id, &delivery_id],
            )
            .await?;
            // Mismatched head invalidates open AWR review approvals for this work.
            invalidate_open_rounds_for_contract(
                &tx,
                tenant_id,
                project_id,
                &work_id,
                &contract_hash,
            )
            .await?;
            tx.commit().await?;
            return Err(PgError::PreconditionsChanged);
        }
        let live = current_contract(&tx, tenant_id, project_id, "main", &work_id).await?;
        if current_contract_hash(&live) != contract_hash {
            tx.execute(
                "UPDATE awr_team.pr_deliveries
                 SET state='invalidated', invalidation_reason='contract_hash_mismatch'
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant_id, &project_id, &delivery_id],
            )
            .await?;
            invalidate_open_rounds_for_contract(
                &tx,
                tenant_id,
                project_id,
                &work_id,
                &contract_hash,
            )
            .await?;
            tx.commit().await?;
            return Err(PgError::PreconditionsChanged);
        }
        if let Some(v) = gh_approved {
            cur_approved = v;
        }
        if let Some(v) = gh_merged {
            cur_merged = v;
        }
        if let Some(m) = merge_sha {
            cur_merge = Some(m.to_owned());
        }
        let observed_ts = validate_observed_at(observed_at)?;
        tx.execute(
            "UPDATE awr_team.pr_deliveries
             SET gh_approved=$4, gh_merged=$5, merge_sha=$6, fact_source=$7, observed_at=$8
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[
                &tenant_id,
                &project_id,
                &delivery_id,
                &cur_approved,
                &cur_merged,
                &cur_merge,
                &fact_source,
                &observed_ts,
            ],
        )
        .await?;
        crate::tx::emit_event(
            &tx,
            tenant_id,
            project_id,
            actor_id,
            &work_id,
            "delivery.pr_observed",
            json!({
                "delivery_id": delivery_id,
                "gh_approved": cur_approved,
                "gh_merged": cur_merged,
                "merge_sha": cur_merge,
                "fact_source": fact_source,
                "observed_at": observed_at,
                "awr_acceptance_complete": false,
                "webhook_auto_sync": false,
            }),
        )
        .await?;
        {
            let summary = json!({
                "delivery_id": delivery_id,
                "expected_head_sha": expected_head_sha,
                "gh_approved": cur_approved,
                "gh_merged": cur_merged,
                "fact_source": fact_source,
            });
            let audit = crate::ops_audit::OpsAuditWrite {
                category: crate::ops_audit::OpsCategory::Delivery,
                action: "delivery.observe_pr".into(),
                result: "committed",
                person_id: None,
                actor_id: actor_id.to_string(),
                client_id: "delivery".into(),
                target_kind: "delivery".into(),
                target_id: Some(delivery_id.to_string()),
                work_id: Some(work_id.clone()),
                change_id: None,
                request_id: None,
                membership_version: None,
                authority_version: None,
                policy_version: Some(awr_team::PERMISSION_POLICY_VERSION as i32),
                source_version: Some(contract_hash.clone()),
                digest: Some(crate::ops_audit::digest_of(&summary)),
                summary,
            };
            crate::ops_audit::record_in_tx(&tx, tenant_id, project_id, &audit).await?;
        }
        tx.commit().await?;
        Ok(PrDelivery {
            id: delivery_id.into(),
            work_id,
            contract_hash,
            repository,
            pr_number,
            pr_url,
            head_sha,
            merge_sha: cur_merge,
            test_evidence_id,
            gh_submitted,
            gh_approved: cur_approved,
            gh_merged: cur_merged,
            fact_source: fact_source.into(),
            observed_at: observed_at.into(),
            state: "active".into(),
        })
    }

    /// Combined status: GitHub PR facts vs AWR acceptance (never conflated).
    pub async fn delivery_status(
        &self,
        tenant_id: &str,
        project_id: &str,
        work_id: &str,
    ) -> PgResult<Value> {
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_scope(&tx, tenant_id, project_id).await?;
        let pr = tx
            .query_opt(
                "SELECT id, repository, pr_number, pr_url, head_sha, merge_sha,
                        gh_submitted, gh_approved, gh_merged, fact_source, observed_at,
                        contract_hash, state, test_evidence_id
                 FROM awr_team.pr_deliveries
                 WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='active'
                 ORDER BY created_at DESC LIMIT 1",
                &[&tenant_id, &project_id, &work_id],
            )
            .await?;
        let runtime: Option<(String, Option<String>)> = tx
            .query_opt(
                "SELECT state, selected_completion_id FROM awr_team.work_runtime
                 WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3",
                &[&tenant_id, &project_id, &work_id],
            )
            .await?
            .map(|r| (r.get(0), r.get(1)));
        let awr_complete = runtime
            .as_ref()
            .is_some_and(|(s, cid)| s == "completed" && cid.is_some());
        let pr = pr.as_ref().map(legacy_pr_snapshot);
        let status = json!({
            "work_id": work_id,
            "pr": pr,
            "neutral_observation": pr.as_ref().map(LegacyPrSnapshot::incomplete_observation),
            "github": {
                "submitted": pr.as_ref().is_some_and(|r| r.submitted),
                "approved": pr.as_ref().is_some_and(|r| r.approved),
                "merged": pr.as_ref().is_some_and(|r| r.merged),
            },
            "awr_acceptance": {
                "complete": awr_complete,
                "runtime_state": runtime.as_ref().map(|(s,_)| s.clone()),
                "selected_completion_id": runtime.as_ref().and_then(|(_,c)| c.clone()),
            },
            "cannot_skip_acceptance_via": [
                "pr_url_alone",
                "green_ci",
                "admin_role",
                "already_merged"
            ],
            "webhook_auto_sync": false,
        });
        tx.commit().await?;
        Ok(status)
    }

    pub async fn source_declared_is_not_complete(
        &self,
        tenant_id: &str,
        project_id: &str,
        work_id: &str,
        scope_id: &str,
    ) -> PgResult<bool> {
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_scope(&tx, tenant_id, project_id).await?;
        let completed: i64 = tx
            .query_one(
                "SELECT count(*) FROM awr_team.completion_receipts
                 WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3",
                &[&tenant_id, &project_id, &work_id],
            )
            .await?
            .get(0);
        let runtime: Option<String> = tx
            .query_opt(
                "SELECT state FROM awr_team.work_runtime
                 WHERE tenant_id=$1 AND project_id=$2 AND scope_id=$3 AND work_id=$4",
                &[&tenant_id, &project_id, &scope_id, &work_id],
            )
            .await?
            .map(|row| row.get(0));
        tx.commit().await?;
        Ok(completed == 0 && runtime.as_deref() != Some("completed"))
    }
}

struct LoadedEvidence {
    work_id: String,
    contract_hash: String,
    digest: String,
    trust_basis: String,
    payload: Value,
    artifact_id: Option<String>,
    output_digest: Option<String>,
    execution_result_digest: Option<String>,
    input_digest: Option<String>,
    execution_id: Option<String>,
    created_by: String,
}

fn assigned_trust(actor_kind: &str, claimed: Option<&str>) -> String {
    match actor_kind {
        "system" => "trusted_executor".into(),
        "human" => "human_review".into(),
        _ => {
            let _ = claimed;
            "caller_asserted".into()
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Canonical evidence digest: bound to work/contract/input/artifact AND
/// computed over canonical JSON, so a jsonb round-trip cannot drift it
/// (CR #42 P2-3).
pub(crate) fn evidence_digest(
    work_id: &str,
    contract_hash: &str,
    input_digest: Option<&str>,
    output_digest: Option<&str>,
    execution_result_digest: Option<&str>,
    payload: &Value,
) -> PgResult<String> {
    let canonical = awr_team::canonical_json(&json!({
        "work_id": work_id,
        "contract_hash": contract_hash,
        "input_digest": input_digest,
        "output_digest": output_digest,
        "execution_result_digest": execution_result_digest,
        "payload": payload,
    }))
    .map_err(|e| PgError::Protocol(e.to_string()))?;
    Ok(sha256_hex(&canonical))
}

fn evidence_bytes_changed(evidence: &LoadedEvidence) -> PgResult<bool> {
    let expected = evidence_digest(
        &evidence.work_id,
        &evidence.contract_hash,
        evidence.input_digest.as_deref(),
        evidence.output_digest.as_deref(),
        evidence.execution_result_digest.as_deref(),
        &evidence.payload,
    )?;
    Ok(expected != evidence.digest)
}

fn current_contract_hash(contract: &Value) -> String {
    contract
        .get("contract_hash")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

async fn lock_project(
    tx: &tokio_postgres::Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
) -> PgResult<()> {
    crate::tx::lock_active_project(tx, tenant_id, project_id).await
}

async fn load_evidence(
    tx: &tokio_postgres::Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    evidence_id: &str,
) -> PgResult<LoadedEvidence> {
    let row = tx
        .query_opt(
            "SELECT work_id, contract_hash, digest, trust_basis, payload_json, artifact_id, output_digest,
                    execution_result_digest, input_digest, execution_id, created_by
             FROM awr_team.evidence
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant_id, &project_id, &evidence_id],
        )
        .await?
        .ok_or(PgError::EvidenceInvalid)?;
    Ok(LoadedEvidence {
        work_id: row.get(0),
        contract_hash: row.get(1),
        digest: row.get(2),
        trust_basis: row.get(3),
        payload: row.get(4),
        artifact_id: row.get(5),
        output_digest: row.get(6),
        execution_result_digest: row.get(7),
        input_digest: row.get(8),
        execution_id: row.get(9),
        created_by: row.get(10),
    })
}

async fn current_contract(
    tx: &tokio_postgres::Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    scope_id: &str,
    work_id: &str,
) -> PgResult<Value> {
    let row = tx
        .query_opt(
            "SELECT c.contract_hash, c.contract_json
             FROM awr_team.projects p
             JOIN awr_team.work_contracts c
               ON c.tenant_id=p.tenant_id AND c.project_id=p.id
              AND c.snapshot_id=p.active_snapshot_id
             WHERE p.tenant_id=$1 AND p.id=$2 AND c.scope_id=$3 AND c.work_id=$4",
            &[&tenant_id, &project_id, &scope_id, &work_id],
        )
        .await?
        .ok_or(PgError::EvidenceInvalid)?;
    let hash: String = row.get(0);
    let mut json: Value = row.get(1);
    json.as_object_mut()
        .map(|map| map.insert("contract_hash".into(), Value::String(hash)));
    Ok(json)
}

/// Required-dependency coverage: every required predecessor of the current
/// contract must have a completion receipt for ITS current contract; the
/// actual (upstream_work, receipt) pairs are returned for the completion
/// mapping. An empty required set passes; "no invalid binding rows" is NOT
/// proof of coverage (CR #42 P2-6). Agent and simulated-member review require
/// an explicit versioned assurance basis on each required predecessor.
pub(crate) async fn required_dependencies_covered(
    tx: &tokio_postgres::Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    work_id: &str,
    scope_id: &str,
    contract: &Value,
) -> PgResult<(bool, Vec<(String, String)>)> {
    let invalid: i64 = tx
        .query_one(
            "SELECT count(*) FROM awr_team.dependency_bindings
             WHERE tenant_id=$1 AND project_id=$2 AND downstream_work_id=$3 AND valid=FALSE",
            &[&tenant_id, &project_id, &work_id],
        )
        .await?
        .get(0);
    if invalid > 0 {
        return Ok((false, vec![]));
    }
    let (required, modes) = dependency_policy(contract)?;
    let mut links = Vec::new();
    for upstream in &required {
        let receipt = tx
            .query_opt(
                "SELECT r.id,r.independence_kind,r.policy,r.approved_by_json,
                    r.contract_hash,r.evidence_id,p.active_snapshot_id,r.execution_id,
                    e.execution_id,r.evidence_bundle_hash,e.digest,r.result_digest
                 FROM awr_team.completion_receipts r
                 JOIN awr_team.work_runtime w
                   ON w.tenant_id=r.tenant_id AND w.project_id=r.project_id
                  AND w.scope_id=r.scope_id AND w.work_id=r.work_id
                  AND w.selected_completion_id=r.id AND w.state='completed'
                 JOIN awr_team.work_contracts c
                   ON c.tenant_id=r.tenant_id AND c.project_id=r.project_id
                  AND c.work_id=r.work_id AND c.scope_id=r.scope_id
                  AND c.contract_hash=r.contract_hash
                 JOIN awr_team.projects p
                   ON p.tenant_id=c.tenant_id AND p.id=c.project_id
                  AND p.active_snapshot_id=c.snapshot_id
                 LEFT JOIN awr_team.evidence e
                   ON e.tenant_id=r.tenant_id AND e.project_id=r.project_id
                  AND e.id=r.evidence_id AND e.work_id=r.work_id
                  AND e.contract_hash=r.contract_hash
                 WHERE r.tenant_id=$1 AND r.project_id=$2 AND r.work_id=$3
                   AND r.scope_id=$4",
                &[&tenant_id, &project_id, upstream, &scope_id],
            )
            .await?;
        match receipt {
            Some(row) => {
                let kind: Option<String> = row.get(1);
                let mode = modes.get(upstream).copied();
                let basis: Value = row.get(3);
                let accepted = dependency_receipt_accepted(
                    mode,
                    kind.as_deref(),
                    &row.get::<_, String>(2),
                    &basis,
                );
                if !accepted {
                    return Ok((false, vec![]));
                }
                if mode == Some(awr_team::DependencyAcceptanceMode::SimulatedMemberIndependent)
                    && !simulated_dependency_verified(
                        tx, tenant_id, project_id, upstream, &row, &basis,
                    )
                    .await?
                {
                    return Ok((false, vec![]));
                }
                links.push((upstream.clone(), row.get(0)));
            }
            None => return Ok((false, vec![])),
        }
    }
    Ok((true, links))
}

pub(crate) async fn simulated_dependency_verified(
    tx: &tokio_postgres::Transaction<'_>,
    tenant: &str,
    project: &str,
    upstream: &str,
    receipt: &tokio_postgres::Row,
    basis: &Value,
) -> PgResult<bool> {
    let (Some(round), Some(decision), Some(evidence)) = (
        basis["review_round_id"].as_str(),
        basis["review_decision_id"].as_str(),
        receipt.get::<_, Option<String>>(5),
    ) else {
        return Ok(false);
    };
    let execution: Option<String> = receipt.get(7);
    let original_execution: Option<String> = receipt.get(8);
    let original_digest: Option<String> = receipt.get(10);
    let evidence_digest: String = receipt.get(9);
    if execution.is_none()
        || execution != original_execution
        || original_digest.as_deref() != Some(evidence_digest.as_str())
        || receipt.get::<_, String>(11) != evidence_digest
    {
        return Ok(false);
    }
    let contract_hash: String = receipt.get(4);
    let snapshot: String = receipt.get(6);
    let actual = simulated_member::verify_completion(
        tx,
        tenant,
        project,
        simulated_member::CompletionBind {
            work: upstream,
            contract_hash: &contract_hash,
            evidence: &evidence,
            round,
            decision,
            snapshot: &snapshot,
        },
    )
    .await;
    match actual {
        Ok(actual) => Ok(basis["member_review_basis"] == actual),
        Err(
            PgError::ReviewRequired
            | PgError::EvidenceInvalid
            | PgError::AuthorCannotReview
            | PgError::Forbidden,
        ) => Ok(false),
        // A failed database read is not an ordinary waiting dependency.
        Err(error) => Err(error),
    }
}

fn dependency_policy(
    contract: &Value,
) -> PgResult<(
    Vec<String>,
    std::collections::BTreeMap<String, awr_team::DependencyAcceptanceMode>,
)> {
    let object = contract.as_object().ok_or(PgError::SourceDivergence)?;
    if !object.contains_key("codec") {
        // Pre-workstream contracts are unversioned documents, not incomplete
        // V1 wire contracts. Keep their dependency gate without introducing an
        // implicit route to the V2 Agent-review assurance policy.
        if object.contains_key("dependency_acceptance") {
            return Err(PgError::SourceDivergence);
        }
        let required: Vec<String> = match object.get("required_dependencies") {
            None => Vec::new(),
            Some(value) => {
                serde_json::from_value(value.clone()).map_err(|_| PgError::SourceDivergence)?
            }
        };
        for work in &required {
            awr_team::WorkId::new(work).map_err(|_| PgError::SourceDivergence)?;
        }
        return Ok((required, Default::default()));
    }
    // The legacy loader adds a hash to its otherwise exact contract document.
    let mut definition = object.clone();
    definition.remove("contract_hash");
    let consumer: awr_team::WorkContract =
        serde_json::from_value(Value::Object(definition)).map_err(|_| PgError::SourceDivergence)?;
    Ok((
        consumer.required_dependencies,
        consumer.dependency_acceptance,
    ))
}

pub(crate) fn dependency_receipt_accepted(
    mode: Option<awr_team::DependencyAcceptanceMode>,
    kind: Option<&str>,
    policy: &str,
    basis: &Value,
) -> bool {
    match mode {
        None => {
            !matches!(kind, Some("agent_review" | "simulated_member_independent"))
                && policy != simulated_member::POLICY
                && basis["approval_basis"] != simulated_member::APPROVAL_BASIS
        }
        Some(awr_team::DependencyAcceptanceMode::AgentReviewedCallerAssertedReconciled) => {
            kind == Some("agent_review")
                && policy == AGENT_REVIEW_POLICY
                && basis["approval_basis"] == "agent_review"
                && basis["execution_basis"] == "caller_asserted_reconciled"
                && basis["human_approval"] == false
                && basis["team_independent_acceptance"] == false
        }
        Some(awr_team::DependencyAcceptanceMode::SimulatedMemberIndependent) => {
            kind == Some(simulated_member::INDEPENDENCE)
                && policy == simulated_member::POLICY
                && basis["approval_basis"] == simulated_member::APPROVAL_BASIS
                && matches!(
                    basis["execution_basis"].as_str(),
                    Some("caller_asserted_reconciled" | "caller_asserted_workspace_settled")
                )
                && basis["human_approval"] == false
                && basis["team_independent_acceptance"] == false
        }
        // A declared cross-stream edge needs an authenticated, version-bound
        // export adoption. This same-stream receipt shortcut cannot grant it.
        Some(awr_team::DependencyAcceptanceMode::CrossWorkstream(_)) => false,
    }
}

#[cfg(test)]
mod dependency_policy_tests {
    use super::*;
    #[test]
    fn cross_stream_declaration_cannot_use_the_same_stream_receipt_shortcut() {
        for review_assurance in [
            awr_team::CrossWorkstreamReviewAssurance::TeamIndependent,
            awr_team::CrossWorkstreamReviewAssurance::SimulatedMemberIndependent,
        ] {
            for version_policy in [
                awr_core::DeliveryVersionPolicy::FixedDelivery,
                awr_core::DeliveryVersionPolicy::CurrentContract,
            ] {
                let mode = Some(awr_team::DependencyAcceptanceMode::CrossWorkstream(
                    awr_team::CrossWorkstreamDependencyPolicy {
                        review_assurance,
                        version_policy,
                    },
                ));
                for kind in [
                    None,
                    Some("team_independent"),
                    Some("agent_review"),
                    Some(simulated_member::INDEPENDENCE),
                ] {
                    for policy in [
                        "independent_review",
                        AGENT_REVIEW_POLICY,
                        simulated_member::POLICY,
                    ] {
                        for basis in [
                            json!({}),
                            json!({
                                "approval_basis":simulated_member::APPROVAL_BASIS,
                                "execution_basis":"caller_asserted_workspace_settled",
                                "human_approval":false,"team_independent_acceptance":false
                            }),
                        ] {
                            assert!(!dependency_receipt_accepted(mode, kind, policy, &basis));
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn reconciled_only_dependency_mode_refuses_workspace_settlement_basis() {
        let basis = json!({"approval_basis":"agent_review","execution_basis":"caller_asserted_workspace_settled",
            "human_approval":false,"team_independent_acceptance":false});
        for mode in [
            None,
            Some(awr_team::DependencyAcceptanceMode::AgentReviewedCallerAssertedReconciled),
        ] {
            assert!(!dependency_receipt_accepted(
                mode,
                Some("agent_review"),
                AGENT_REVIEW_POLICY,
                &basis
            ));
        }
    }
    #[test]
    fn unversioned_contracts_preserve_dependencies_without_v2_opt_in() {
        let (required, modes) = dependency_policy(&json!({
            "completion_policy":"independent_review", "acceptance":["verified"],
            "required_dependencies":["upstream"], "contract_hash":"legacy-hash"
        }))
        .unwrap();
        assert_eq!(required, ["upstream"]);
        assert!(modes.is_empty());
        assert!(
            dependency_policy(&json!({"completion_policy":"ordinary_confirm"}))
                .unwrap()
                .0
                .is_empty()
        );
        for required in [Value::Null, json!("upstream"), json!([1]), json!([""])] {
            assert!(dependency_policy(&json!({"required_dependencies":required})).is_err());
        }
        for mode in [
            Value::Null,
            json!({}),
            json!({"upstream":"agent_reviewed_caller_asserted_reconciled"}),
        ] {
            assert!(
                dependency_policy(
                    &json!({"required_dependencies":["upstream"],"dependency_acceptance":mode})
                )
                .is_err()
            );
        }
        assert!(dependency_policy(&json!({"codec":"unknown"})).is_err());
        assert!(dependency_policy(&json!({"codec":null})).is_err());
        assert!(dependency_policy(&json!({"codec":awr_team::WorkContract::CODEC})).is_err());
    }

    #[test]
    fn versioned_dependency_policy_keeps_closed_v1_and_explicit_v2() {
        let mut contract = json!({"codec":awr_team::WorkContract::CODEC,
            "work_id":"consumer","external_key":"consumer","goals":["ship"],
            "hard_rules":[],"scope_paths":["src"],"acceptance":["verified"],
            "required_dependencies":["upstream"],"completion_policy":"independent_review",
            "verification_requirements":[],"contract_hash":"stored-hash"});
        assert!(dependency_policy(&contract).unwrap().1.is_empty());
        contract["dependency_acceptance"] =
            json!({"upstream":"agent_reviewed_caller_asserted_reconciled"});
        assert!(dependency_policy(&contract).is_err());
        contract["codec"] = json!(awr_team::WorkContract::CODEC_V2);
        let (required, modes) = dependency_policy(&contract).unwrap();
        assert_eq!(required, ["upstream"]);
        assert_eq!(
            modes["upstream"],
            awr_team::DependencyAcceptanceMode::AgentReviewedCallerAssertedReconciled
        );
        contract["unrecognized_field"] = json!(true);
        assert!(dependency_policy(&contract).is_err());
    }

    #[test]
    fn simulated_dependency_metadata_requires_opt_in_without_human_uplift() {
        let mode = Some(awr_team::DependencyAcceptanceMode::SimulatedMemberIndependent);
        for execution in [
            "caller_asserted_reconciled",
            "caller_asserted_workspace_settled",
        ] {
            let basis = json!({"approval_basis":simulated_member::APPROVAL_BASIS,
                "execution_basis":execution,"human_approval":false,"team_independent_acceptance":false});
            assert!(dependency_receipt_accepted(
                mode,
                Some(simulated_member::INDEPENDENCE),
                simulated_member::POLICY,
                &basis
            ));
            assert!(!dependency_receipt_accepted(
                None,
                Some(simulated_member::INDEPENDENCE),
                simulated_member::POLICY,
                &basis
            ));
            assert!(!dependency_receipt_accepted(
                Some(awr_team::DependencyAcceptanceMode::AgentReviewedCallerAssertedReconciled),
                Some(simulated_member::INDEPENDENCE),
                simulated_member::POLICY,
                &basis
            ));
            for field in ["human_approval", "team_independent_acceptance"] {
                for value in [json!(true), Value::Null] {
                    let mut changed = basis.clone();
                    changed[field] = value;
                    assert!(!dependency_receipt_accepted(
                        mode,
                        Some(simulated_member::INDEPENDENCE),
                        simulated_member::POLICY,
                        &changed
                    ));
                }
            }
            let mut changed = basis.clone();
            changed["execution_basis"] = json!("server_attested");
            assert!(!dependency_receipt_accepted(
                mode,
                Some(simulated_member::INDEPENDENCE),
                simulated_member::POLICY,
                &changed
            ));
            for kind in [None, Some("agent_review"), Some("team_independent")] {
                assert!(!dependency_receipt_accepted(
                    None,
                    kind,
                    simulated_member::POLICY,
                    &basis
                ));
                assert!(!dependency_receipt_accepted(
                    None,
                    kind,
                    "ordinary_confirm",
                    &basis
                ));
                assert!(!dependency_receipt_accepted(
                    mode,
                    kind,
                    simulated_member::POLICY,
                    &basis
                ));
            }
            assert!(!dependency_receipt_accepted(
                mode,
                Some(simulated_member::INDEPENDENCE),
                AGENT_REVIEW_POLICY,
                &basis
            ));
        }
    }

    #[test]
    fn mapped_receipt_requires_the_entire_basis_tuple_and_unmapped_keeps_legacy_rule() {
        let mode = Some(awr_team::DependencyAcceptanceMode::AgentReviewedCallerAssertedReconciled);
        let basis = json!({"approval_basis":"agent_review","execution_basis":"caller_asserted_reconciled",
            "human_approval":false,"team_independent_acceptance":false});
        assert!(dependency_receipt_accepted(
            mode,
            Some("agent_review"),
            AGENT_REVIEW_POLICY,
            &basis
        ));
        assert!(!dependency_receipt_accepted(
            None,
            Some("agent_review"),
            AGENT_REVIEW_POLICY,
            &basis
        ));
        for kind in [None, Some("ordinary"), Some("team_independent")] {
            assert!(dependency_receipt_accepted(
                None,
                kind,
                "review",
                &json!({})
            ));
            assert!(!dependency_receipt_accepted(
                mode,
                kind,
                AGENT_REVIEW_POLICY,
                &basis
            ));
        }
        assert!(!dependency_receipt_accepted(
            mode,
            Some("agent_review"),
            "independent_review",
            &basis
        ));
        for key in [
            "approval_basis",
            "execution_basis",
            "human_approval",
            "team_independent_acceptance",
        ] {
            for value in [
                Value::Null,
                json!(true),
                json!("false"),
                json!("trusted_execution"),
            ] {
                let mut changed = basis.clone();
                changed[key] = value;
                assert!(
                    !dependency_receipt_accepted(
                        mode,
                        Some("agent_review"),
                        AGENT_REVIEW_POLICY,
                        &changed
                    ),
                    "{key}"
                );
            }
        }
    }
}

async fn load_operation(
    tx: &tokio_postgres::Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    actor_id: &str,
    client_id: &str,
    request_id: &str,
) -> PgResult<Option<(String, Value)>> {
    let row = tx
        .query_opt(
            "SELECT request_hash, result_json FROM awr_team.operations
             WHERE tenant_id=$1 AND project_id=$2 AND actor_id=$3 AND client_id=$4 AND request_id=$5",
            &[&tenant_id, &project_id, &actor_id, &client_id, &request_id],
        )
        .await?;
    Ok(row.map(|row| (row.get(0), row.get(1))))
}

async fn store_operation(
    tx: &tokio_postgres::Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    actor_id: &str,
    client_id: &str,
    request_id: &str,
    op: &str,
    request_hash: &str,
    committed_revision: i64,
    result: &Value,
) -> PgResult<()> {
    tx.execute(
        "INSERT INTO awr_team.operations(
            tenant_id, project_id, id, actor_id, client_id, request_id, op,
            request_hash, state, committed_project_revision, result_json)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'committed',$9,$10)",
        &[
            &tenant_id,
            &project_id,
            &new_id(),
            &actor_id,
            &client_id,
            &request_id,
            &op,
            &request_hash,
            &committed_revision,
            result,
        ],
    )
    .await?;
    Ok(())
}

async fn current_review(
    tx: &tokio_postgres::Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    work_id: &str,
    bundle_hash: &str,
) -> PgResult<ReviewPolicy> {
    let row = tx
        .query_opt(
            "SELECT id, author_actor_id, state FROM awr_team.review_rounds
             WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND bundle_hash=$4
             ORDER BY round_index DESC LIMIT 1",
            &[&tenant_id, &project_id, &work_id, &bundle_hash],
        )
        .await?;
    match row {
        None => Ok(ReviewPolicy {
            required: true,
            author_may_self_approve: false,
            approved: false,
            reviewer_is_author: false,
        }),
        Some(row) => {
            let round_id: String = row.get(0);
            let author: String = row.get(1);
            let state: String = row.get(2);
            // Pin the round first, then read ITS decisions only: the latest
            // round and the latest decision must not be picked from
            // different rounds (CR #42 P2-4).
            let reviewer: Option<String> = tx
                .query_opt(
                    "SELECT reviewer_actor_id FROM awr_team.review_decisions d
                     WHERE d.tenant_id=$1 AND d.project_id=$2 AND d.review_round_id=$3
                     ORDER BY d.created_at DESC LIMIT 1",
                    &[&tenant_id, &project_id, &round_id],
                )
                .await?
                .map(|row| row.get(0));
            Ok(ReviewPolicy {
                required: true,
                author_may_self_approve: false,
                approved: state == "approved",
                reviewer_is_author: reviewer.as_deref() == Some(author.as_str()),
            })
        }
    }
}

/// Resolve the responsible person for an actor.
/// Humans map to a persons row with the same id (created if needed).
/// Agents require an active person_agent_bindings row — another agent of the
/// same person is still that person (WS-018 independence).
pub(crate) async fn resolve_person_id(
    tx: &tokio_postgres::Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    actor_id: &str,
) -> PgResult<String> {
    let kind: String = tx
        .query_opt(
            "SELECT kind FROM awr_team.actors WHERE tenant_id=$1 AND id=$2",
            &[&tenant_id, &actor_id],
        )
        .await?
        .map(|r| r.get(0))
        .ok_or(PgError::Forbidden)?;
    if let Some(row) = tx
        .query_opt(
            "SELECT person_id FROM awr_team.person_agent_bindings
             WHERE tenant_id=$1 AND project_id=$2 AND agent_id=$3 AND status='active'",
            &[&tenant_id, &project_id, &actor_id],
        )
        .await?
    {
        return Ok(row.get(0));
    }
    if kind == "human" {
        tx.execute(
            "INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
             VALUES ($1,$2,$3,$3,'active')
             ON CONFLICT (tenant_id, project_id, id) DO NOTHING",
            &[&tenant_id, &project_id, &actor_id],
        )
        .await?;
        return Ok(actor_id.to_owned());
    }
    Err(PgError::Forbidden)
}

pub(crate) fn self_review_permitted(completion_policy: &str) -> bool {
    matches!(
        completion_policy,
        "trusted_execution_and_author_self_review"
    )
}

fn is_git_sha(s: &str) -> bool {
    s.len() == 40
        && s.bytes().all(|b| b.is_ascii_hexdigit())
        && s.bytes().all(|b| !b.is_ascii_uppercase())
}

fn validate_observed_at(raw: &str) -> PgResult<&str> {
    // Accept RFC3339-like timestamps; PG casts to timestamptz. Reject empties/controls.
    let s = raw.trim();
    if s.is_empty() || s.len() > 64 || s.chars().any(char::is_control) {
        return Err(PgError::Protocol("observed_at must be RFC3339".into()));
    }
    // Minimal shape: YYYY-MM-DDThh:mm:ss...Z or with offset
    let bytes = s.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return Err(PgError::Protocol("observed_at must be RFC3339".into()));
    }
    Ok(s)
}

async fn invalidate_open_rounds_for_contract(
    tx: &tokio_postgres::Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    work_id: &str,
    contract_hash: &str,
) -> PgResult<()> {
    // Keep rejected/approved history; only open rounds for mismatched contract are invalidated.
    tx.execute(
        "UPDATE awr_team.review_rounds SET state='invalidated'
         WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3
           AND state='open' AND contract_hash<>$4",
        &[&tenant_id, &project_id, &work_id, &contract_hash],
    )
    .await?;
    Ok(())
}

async fn invalidate_open_rounds(
    tx: &tokio_postgres::Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    work_id: &str,
    new_bundle: &str,
) -> PgResult<()> {
    tx.execute(
        "UPDATE awr_team.review_rounds SET state='invalidated'
         WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3
           AND state IN ('open','approved') AND bundle_hash <> $4",
        &[&tenant_id, &project_id, &work_id, &new_bundle],
    )
    .await?;
    Ok(())
}
