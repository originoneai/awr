//! Authoritative planning writeback and consistent activation (AWR-TMCP-022).
//!
//! Consumes TMCP-021 publish receipts (`source_writeback_pending`), writes
//! approved changes into the bound sole source under WS-022 fingerprint/
//! recovery semantics, and activates one coherent PG snapshot. Admission uses
//! server-derived projection impact and persisted execution settlement before
//! writing, then fences affected effects and dependency changes until commit.
use super::{IngestRequest, PgError, PgResult, SourceFile, SourceStore};
use crate::tx::{bind_workstream_scope, lock_active_project, new_id};
use crate::workstream_auth::{authenticate, authenticate_writer};
#[allow(unused_imports)]
use awr_source::SOURCE_BINDING_FILE;
use awr_source::{
    LockedSourceFile, PublishPrepOptions, SoleSourceLocation, apply_planning_changes_to_ledger,
    fingerprint, prepare_publish_from_ledger_bytes, refuse_external_overwrite,
    source_status_notes_are_completion_receipts,
};
use awr_team::{DraftChange, SourceActivationPlan};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;
use tokio_postgres::Transaction;

/// Compatibility shape for impact observations. Positive caller declarations
/// never grant admission; source activation independently derives actual impact.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationImpactGate {
    pub impact_proven: bool,
    pub allow_activation: bool,
    pub affected_work_ids: Vec<String>,
    pub unrelated_work_ids: Vec<String>,
    /// Retained wire field; caller stop assertions are not settlement evidence.
    pub stopped_work_ids: Vec<String>,
    pub refuse_reason: Option<String>,
    pub recovery_actions: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WritebackActivateRequest {
    pub request_id: String,
    pub publish_receipt_id: String,
    /// Absolute path to the sole-source server directory (or checkout).
    pub source_root: PathBuf,
    /// Ledger path relative to `source_root`.
    pub ledger_relative_path: String,
    /// False is an explicit veto. True does not prove impact or settlement.
    pub impact_proven: bool,
    /// Retained original-request metadata; never authority to stop or activate.
    #[serde(default)]
    pub stopped_work_ids: Vec<String>,
}

#[path = "source_writeback/admission.rs"]
pub(crate) mod admission;
#[path = "source_writeback/journal.rs"]
pub(crate) mod journal;
#[path = "source_writeback/request_binding.rs"]
mod request_binding;

impl SourceStore {
    /// Resume the same exact intent after inspecting physical source and PG state.
    pub async fn activate_planning_writeback(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        req: &WritebackActivateRequest,
    ) -> PgResult<Value> {
        self.activate_planning_writeback_inner(tenant_id, project_id, bearer, req, None)
            .await
    }

    /// Exercise real durable interruption boundaries; never a transport operation.
    #[cfg(feature = "pg-tests")]
    pub async fn activate_planning_writeback_abort_for_test(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        req: &WritebackActivateRequest,
        boundary: &str,
    ) -> PgResult<Value> {
        self.activate_planning_writeback_inner(tenant_id, project_id, bearer, req, Some(boundary))
            .await
    }

    pub(super) async fn activate_planning_writeback_inner(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        req: &WritebackActivateRequest,
        abort: Option<&str>,
    ) -> PgResult<Value> {
        use request_binding::Intent;
        if req.request_id.trim().is_empty() || req.request_id.len() > 200 {
            return Err(PgError::Protocol(
                "request_id required for idempotent writeback".into(),
            ));
        }
        assert!(!source_status_notes_are_completion_receipts());
        let mut client = self.connect().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let auth = writer(&tx, tenant, project, bearer).await?;
        let prior = journal::load(&tx, tenant, project, &req.request_id).await?;
        if let Some(prior) = &prior {
            prior.intent.require_original(req, &auth)?;
            if prior.phase == "completed" {
                let id = prior
                    .receipt_id
                    .as_deref()
                    .ok_or(PgError::SourceDivergence)?;
                let confirmed = journal::outcome_in_tx(&tx, tenant, project, &req.request_id)
                    .await?
                    .is_some_and(|outcome| {
                        outcome["applied"] == true && outcome["activation_receipt_id"] == id
                    });
                if !confirmed {
                    return Err(PgError::SourceDivergence);
                }
                let receipt = load_activation_receipt(&tx, tenant, project, id).await?;
                tx.commit().await?;
                return Ok(json!({"already_recorded":true,"receipt":receipt}));
            }
            if prior.phase == "refused" {
                return Err(PgError::ActivationImpactUnproven("the original request explicitly refused activation; use a new reviewed request after inspection".into()));
            }
            if !matches!(
                prior.phase.as_str(),
                "validated" | "source_written" | "pg_activating"
            ) {
                return Err(PgError::WritebackRefused(
                    "original journal is not safely resumable".into(),
                ));
            }
        }
        admission::require_source_available(&tx, tenant, project, Some(&req.request_id)).await?;
        let (epoch, registered) = request_binding::baseline(&tx, tenant, project, req).await?;
        let (publication, changes) =
            request_binding::publication(&tx, tenant, project, &auth, &req.publish_receipt_id)
                .await?;
        let mut intent = if let Some(prior) = &prior {
            prior.intent.require_baseline(&auth, &epoch, &registered)?;
            if json!(prior.intent.publication) != json!(publication) {
                return Err(PgError::StaleApproval);
            }
            prior.intent.clone()
        } else {
            let (planning_request, planning_request_hash) =
                request_binding::command_origin(&tx, tenant, project, &auth, req, &publication)
                    .await?;
            Intent {
                codec: "awr-planning-writeback-intent-v1".into(),
                tenant_id: tenant.into(),
                project_id: project.into(),
                request: req.clone(),
                planning_request,
                planning_request_hash,
                actor_id: auth.actor_id.clone(),
                client_id: auth.client_id.clone(),
                coordinator_epoch: auth.epoch.clone(),
                publication,
                base_snapshot_id: auth.snapshot.clone(),
                base_authority_epoch: epoch,
                registered_source: registered,
                before_fingerprint: String::new(),
                after_fingerprint: String::new(),
                source_version: String::new(),
                candidate: None,
                affected_work_ids: vec![],
                dependency_work_ids: vec![],
            }
        };
        // False remains an explicit veto; true and stopped lists are never proof.
        if !req.impact_proven {
            let gate = ActivationImpactGate {
                refuse_reason: Some("caller explicitly refused impact admission".into()),
                recovery_actions: vec![
                    "inspect the actual candidate and submit a new reviewed request".into(),
                ],
                ..Default::default()
            };
            journal::insert(&tx, tenant, project, &intent, &gate, "refused").await?;
            tx.commit().await?;
            return Err(PgError::ActivationImpactUnproven(
                "caller explicitly refused activation".into(),
            ));
        }
        let location =
            SoleSourceLocation::server_directory(&req.source_root, &req.ledger_relative_path)
                .map_err(|_| PgError::Protocol("invalid sole source location".into()))?;
        let guard = LockedSourceFile::open(&req.source_root, &req.ledger_relative_path)
            .map_err(source_write_error)?;
        let disk = guard.read().map_err(source_write_error)?;
        let observed = fingerprint(&disk);
        let (before_fp, after_fp, after_bytes, source_written) = if prior.is_some() {
            if observed == intent.after_fingerprint {
                (
                    intent.before_fingerprint.clone(),
                    intent.after_fingerprint.clone(),
                    disk,
                    true,
                )
            } else if observed == intent.before_fingerprint {
                let patch = apply_planning_changes_to_ledger(&disk, &changes)
                    .map_err(|_| PgError::SourceDivergence)?;
                if patch.after_fingerprint != intent.after_fingerprint {
                    return Err(PgError::SourceDivergence);
                }
                (
                    patch.before_fingerprint,
                    patch.after_fingerprint,
                    patch.after_bytes,
                    false,
                )
            } else {
                return Err(PgError::WritebackRefused("authoritative source changed; preserve the external bytes and inspect the original intent".into()));
            }
        } else {
            let patch = apply_planning_changes_to_ledger(&disk, &changes)
                .map_err(|e| PgError::Protocol(e.to_string()))?;
            (
                patch.before_fingerprint,
                patch.after_fingerprint,
                patch.after_bytes,
                false,
            )
        };
        let package = prepare_publish_from_ledger_bytes(
            &location,
            &req.source_root,
            &after_bytes,
            project,
            &PublishPrepOptions::default(),
        )
        .map_err(|e| PgError::Protocol(e.to_string()))?;
        let files: Vec<SourceFile> = package
            .files
            .iter()
            .map(|f| SourceFile {
                path: f.path.clone(),
                bytes: f.bytes.clone(),
            })
            .collect();
        Self::validate_publish_package(&files)?;
        let raw_files: Vec<_> = files
            .iter()
            .map(|f| (f.path.clone(), f.bytes.clone()))
            .collect();
        let projection = super::SourceProjection::parse(&raw_files, project)?;
        super::workstreams::reject_external_graph(&raw_files)?;
        // This is the same server-derived gate as every direct source activation.
        let affected = projection
            .validate_transition_with_impact(&tx, tenant, project, Some(&auth.snapshot), None)
            .await?;
        let influence = projection
            .dependency_influence(&tx, tenant, project, Some(&auth.snapshot))
            .await?;
        let all = tx
            .query(
                "SELECT id FROM awr_team.work_items WHERE tenant_id=$1 AND project_id=$2",
                &[&tenant, &project],
            )
            .await?;
        let gate = ActivationImpactGate {
            impact_proven: true,
            allow_activation: true,
            affected_work_ids: affected.iter().cloned().collect(),
            unrelated_work_ids: all
                .iter()
                .map(|r| r.get::<_, String>(0))
                .filter(|id| !affected.contains(id))
                .collect(),
            stopped_work_ids: vec![],
            refuse_reason: None,
            recovery_actions: vec![
                "query the durable phase before resuming the original request".into(),
            ],
        };
        if prior.is_some() {
            if intent.affected_work_ids != gate.affected_work_ids
                || intent.dependency_work_ids != influence.into_iter().collect::<Vec<_>>()
                || intent.source_version != package.source_version_digest
            {
                return Err(PgError::SourceDivergence);
            };
            let manifest = super::build_manifest(&package.parser_version, &raw_files)?;
            let digest = super::sha256_hex(
                &serde_json::to_vec(&manifest).map_err(|_| PgError::SourceDivergence)?,
            );
            if intent
                .candidate
                .as_ref()
                .is_none_or(|c| c.manifest_digest != digest)
            {
                return Err(PgError::SourceDivergence);
            };
        } else {
            intent.before_fingerprint = before_fp.clone();
            intent.after_fingerprint = after_fp.clone();
            intent.source_version = package.source_version_digest.clone();
            intent.affected_work_ids = gate.affected_work_ids.clone();
            intent.dependency_work_ids = influence.into_iter().collect();
            let candidate = Self::ingest_in_tx(
                &tx,
                IngestRequest {
                    tenant_id: tenant.into(),
                    project_id: project.into(),
                    actor_id: auth.actor_id.clone(),
                    parser_version: package.parser_version.clone(),
                    files,
                },
            )
            .await?;
            Self::record_writeback_source_approval_in_tx(
                &tx,
                tenant,
                project,
                &candidate.proposal_id,
                &candidate.manifest_digest,
                &intent.publication.approver_actor_id,
                &intent.publication.approval_id,
            )
            .await?;
            intent.candidate = Some(candidate);
            journal::insert(&tx, tenant, project, &intent, &gate, "validated").await?;
        }
        let intent_hash = intent.hash()?;
        let mut phase = prior.map(|p| p.phase).unwrap_or_else(|| "validated".into());
        tx.commit().await?;
        injected(abort, "after_intent")?;
        if !source_written {
            if phase != "validated" {
                return Err(PgError::SourceDivergence);
            }
            guard
                .replace(&before_fp, &after_bytes)
                .map_err(source_write_error)?;
        }
        refuse_external_overwrite(
            &after_fp,
            &fingerprint(&guard.read().map_err(source_write_error)?),
        )
        .map_err(|_| {
            PgError::WritebackRefused("source fingerprint changed before confirmation".into())
        })?;
        injected(abort, "after_source_write")?;
        for next in ["source_written", "pg_activating"] {
            if phase == "pg_activating" || phase == next {
                continue;
            }
            let mut client = self.connect().await?;
            let tx = client.transaction().await?;
            bind_workstream_scope(&tx, tenant, project).await?;
            lock_active_project(&tx, tenant, project).await?;
            journal::transition(&tx, tenant, project, &req.request_id, &intent_hash, next).await?;
            tx.commit().await?;
            phase = next.into();
            injected(
                abort,
                if next == "source_written" {
                    "after_source_written"
                } else {
                    "after_pg_activating"
                },
            )?;
        }
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        let auth = writer(&tx, tenant, project, bearer).await?;
        let saved = journal::load(&tx, tenant, project, &req.request_id)
            .await?
            .ok_or(PgError::SourceDivergence)?;
        saved.intent.require_original(req, &auth)?;
        if saved.hash != intent_hash || saved.phase != "pg_activating" {
            return Err(PgError::SourceDivergence);
        };
        let (epoch, registered) = request_binding::baseline(&tx, tenant, project, req).await?;
        intent.require_baseline(&auth, &epoch, &registered)?;
        let (publication, _) =
            request_binding::publication(&tx, tenant, project, &auth, &req.publish_receipt_id)
                .await?;
        if json!(publication) != json!(intent.publication) {
            return Err(PgError::StaleApproval);
        };
        refuse_external_overwrite(
            &after_fp,
            &fingerprint(&guard.read().map_err(source_write_error)?),
        )
        .map_err(|_| PgError::WritebackRefused("source changed before atomic activation".into()))?;
        let candidate = intent.candidate.as_ref().ok_or(PgError::SourceDivergence)?;
        let current = self
            .activate_in_tx(
                &tx,
                tenant,
                project,
                &auth.actor_id,
                &candidate.proposal_id,
                &SourceActivationPlan {
                    candidate_digest: candidate.manifest_digest.clone(),
                    parser_version: candidate.parser_version.clone(),
                    expected_authority_epoch: candidate.base_epoch.clone(),
                    approved_candidate_digest: candidate.manifest_digest.clone(),
                },
                true,
                false,
                None,
                Some(&req.request_id),
            )
            .await?;
        let id = new_id();
        let p = &intent.publication;
        let audit = json!({"request_id":req.request_id,"intent_hash":intent_hash,"actor_id":intent.actor_id,"client_id":intent.client_id,
            "publish_receipt_id":req.publish_receipt_id,"candidate_id":p.candidate_id,"candidate_digest":p.candidate_digest,
            "approval_id":p.approval_id,"approver_actor_id":p.approver_actor_id,"publisher_actor_id":p.publisher_actor_id,
            "source_version":intent.source_version,"activated_snapshot_id":current.snapshot_id,"authority_epoch":current.authority_epoch,
            "affected_work_ids":intent.affected_work_ids,"unrelated_work_ids":gate.unrelated_work_ids,
            "before_fingerprint":before_fp,"after_fingerprint":after_fp,"source_synchronized_at_commit":true});
        tx.execute("INSERT INTO awr_team.planning_activation_receipts(
            tenant_id,project_id,id,request_id,candidate_id,candidate_digest,publish_receipt_id,approval_id,
            approver_actor_id,publisher_actor_id,source_version,activated_snapshot_id,authority_epoch,
            before_fingerprint,after_fingerprint,affected_work_ids,unrelated_work_ids,audit_json)
            VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)",
            &[&tenant,&project,&id,&req.request_id,&p.candidate_id,&p.candidate_digest,&req.publish_receipt_id,&p.approval_id,
              &p.approver_actor_id,&p.publisher_actor_id,&intent.source_version,&current.snapshot_id,&current.authority_epoch,
              &before_fp,&after_fp,&json!(intent.affected_work_ids),&json!(gate.unrelated_work_ids),&audit]).await?;
        if tx.execute("UPDATE awr_team.planning_publish_receipts SET source_writeback_pending=false,activation_receipt_id=$4,
            source_version=$5,activated_snapshot_id=$6 WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND source_writeback_pending",
            &[&tenant,&project,&req.publish_receipt_id,&id,&intent.source_version,&current.snapshot_id]).await? != 1 {return Err(PgError::PreconditionsChanged)};
        if tx.execute("UPDATE awr_team.planning_writeback_journals SET phase='completed',audit_receipt_id=$4,source_version=$5,
            activated_snapshot_id=$6,authority_epoch=$7,body_json=$8,updated_at=clock_timestamp()
            WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3 AND intent_hash=$9 AND phase='pg_activating'",
            &[&tenant,&project,&req.request_id,&id,&intent.source_version,&current.snapshot_id,&current.authority_epoch,&audit,&intent_hash]).await? != 1 {return Err(PgError::PreconditionsChanged)};
        let mut receipt = load_activation_receipt(&tx, tenant, project, &id).await?;
        injected(abort, "before_final_commit")?;
        tx.commit().await?;
        injected(abort, "after_final_commit")?;
        receipt["already_recorded"] = json!(false);
        receipt["source_bytes_written"] = json!(true);
        receipt["source_writeback_pending"] = json!(false);
        Ok(receipt)
    }

    pub async fn get_planning_writeback_status(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        request: &str,
    ) -> PgResult<Option<Value>> {
        let mut client = self.connect().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let mut auth = authenticate(&tx, tenant, project, bearer).await?;
        crate::delegation_auth::authorize_project_action(
            &tx,
            &mut auth,
            project,
            awr_team::Action::PlanningPublish,
        )
        .await?;
        let outcome = journal::outcome_in_tx(&tx, tenant, project, request).await?;
        tx.commit().await?;
        Ok(outcome)
    }

    async fn record_writeback_source_approval_in_tx(
        tx: &Transaction<'_>,
        tenant_id: &str,
        project_id: &str,
        proposal_id: &str,
        candidate_digest: &str,
        approver_actor_id: &str,
        planning_approval_id: &str,
    ) -> PgResult<String> {
        crate::tx::bind_workstream_scope(tx, tenant_id, project_id).await?;
        lock_active_project(tx, tenant_id, project_id).await?;
        let row = tx
            .query_opt(
                "SELECT p.state, s.manifest_digest
                 FROM awr_team.source_proposals p
                 JOIN awr_team.source_snapshots s
                   ON s.tenant_id=p.tenant_id AND s.project_id=p.project_id
                  AND s.id=p.candidate_snapshot_id
                 WHERE p.tenant_id=$1 AND p.project_id=$2 AND p.id=$3
                 FOR UPDATE OF p",
                &[&tenant_id, &project_id, &proposal_id],
            )
            .await?
            .ok_or_else(|| PgError::Protocol("writeback proposal not found".into()))?;
        let state: String = row.get(0);
        let digest: String = row.get(1);
        if digest != candidate_digest {
            return Err(PgError::StaleApproval);
        }
        if state != "pending" && state != "approved" {
            return Err(PgError::CandidateNotApproved);
        }
        let plan_exists: bool = tx
            .query_one(
                "SELECT EXISTS(
                    SELECT 1 FROM awr_team.planning_approvals
                    WHERE tenant_id=$1 AND project_id=$2 AND id=$3)",
                &[&tenant_id, &project_id, &planning_approval_id],
            )
            .await?
            .get(0);
        if !plan_exists {
            return Err(PgError::CandidateNotApproved);
        }
        let approval_id = new_id();
        tx.execute(
            "INSERT INTO awr_team.source_approvals(
                tenant_id, project_id, id, proposal_id, candidate_digest,
                reviewer_actor_id, decision)
             VALUES ($1,$2,$3,$4,$5,$6,'approve')",
            &[
                &tenant_id,
                &project_id,
                &approval_id,
                &proposal_id,
                &candidate_digest,
                &approver_actor_id,
            ],
        )
        .await?;
        tx.execute(
            "UPDATE awr_team.source_proposals SET state='approved'
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant_id, &project_id, &proposal_id],
        )
        .await?;
        Ok(approval_id)
    }

    /// Query activation / audit receipt by original request id.
    pub async fn get_planning_activation_receipt(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        request_id: &str,
    ) -> PgResult<Option<Value>> {
        let mut client = self.connect().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let mut auth = authenticate(&tx, tenant_id, project_id, bearer).await?;
        if crate::delegation_auth::authorize_project_action(
            &tx,
            &mut auth,
            project_id,
            awr_team::Action::PlanningPublish,
        )
        .await
        .is_err()
        {
            crate::delegation_auth::authorize_project_action(
                &tx,
                &mut auth,
                project_id,
                awr_team::Action::WorkRead,
            )
            .await?;
        }
        let row = tx
            .query_opt(
                "SELECT id FROM awr_team.planning_activation_receipts
                 WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
                &[&tenant_id, &project_id, &request_id],
            )
            .await?;
        let Some(row) = row else {
            tx.commit().await?;
            return Ok(None);
        };
        let id: String = row.get(0);
        let receipt = load_activation_receipt(&tx, tenant_id, project_id, &id).await?;
        tx.commit().await?;
        Ok(Some(receipt))
    }

    pub fn planning_writeback_capabilities() -> Value {
        json!({"source_writeback":"tmcp_022","precise_patch_fingerprint_recovery":true,
            "barrier_lock_order":"ws023","selective_replan":"ws032",
            "project_claim_barrier_retained_when_unproven":true,
            "cancel_expiry_session_end_prove_process_stopped":false,
            "runtime_fields_writable_via_source":false,"source_status_is_completion_receipt":false,
            "idempotent_request_id":true,"queryable_activation_receipt":true,
            "server_derived_preflight_before_source_write":true,"durable_selective_admission":true,
            "original_request_identity_bound":true,"atomic_activation_and_receipt":true})
    }
}

async fn writer(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    bearer: &str,
) -> PgResult<crate::workstream_auth::ReaderAuthority> {
    bind_workstream_scope(tx, tenant, project).await?;
    // Retain source activation's mode -> project lock order.
    tx.query_opt("SELECT enabled FROM awr_team.workstream_modes WHERE tenant_id=$1 AND project_id=$2 FOR UPDATE",
        &[&tenant,&project]).await?.ok_or(PgError::ProjectNotAvailable)?;
    lock_active_project(tx, tenant, project).await?;
    let mut auth = authenticate_writer(tx, tenant, project, bearer).await?;
    crate::delegation_auth::authorize_project_action(
        tx,
        &mut auth,
        project,
        awr_team::Action::PlanningPublish,
    )
    .await?;
    Ok(auth)
}

fn injected(selected: Option<&str>, boundary: &str) -> PgResult<()> {
    if selected == Some(boundary) {
        return Err(PgError::Protocol(format!(
            "injected writeback interruption: {boundary}"
        )));
    }
    Ok(())
}

async fn load_activation_receipt(
    tx: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    id: &str,
) -> PgResult<Value> {
    let row = tx
        .query_one(
            "SELECT id, request_id, candidate_id, candidate_digest, publish_receipt_id,
                    approval_id, approver_actor_id, publisher_actor_id, source_version,
                    activated_snapshot_id, authority_epoch, before_fingerprint, after_fingerprint,
                    affected_work_ids, unrelated_work_ids, audit_json, created_at::text
             FROM awr_team.planning_activation_receipts
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant_id, &project_id, &id],
        )
        .await?;
    Ok(json!({
        "receipt_id": row.get::<_, String>(0),
        "request_id": row.get::<_, String>(1),
        "candidate_id": row.get::<_, String>(2),
        "candidate_digest": row.get::<_, String>(3),
        "publish_receipt_id": row.get::<_, String>(4),
        "approval_id": row.get::<_, String>(5),
        "approver_actor_id": row.get::<_, String>(6),
        "publisher_actor_id": row.get::<_, String>(7),
        "source_version": row.get::<_, String>(8),
        "activated_snapshot_id": row.get::<_, String>(9),
        "authority_epoch": row.get::<_, String>(10),
        "before_fingerprint": row.get::<_, String>(11),
        "after_fingerprint": row.get::<_, String>(12),
        "affected_work_ids": row.get::<_, Value>(13),
        "unrelated_work_ids": row.get::<_, Value>(14),
        "audit": row.get::<_, Value>(15),
        "created_at": row.get::<_, String>(16),
    }))
}

fn source_write_error(error: awr_source::Error) -> PgError {
    match error {
        awr_source::Error::Io(error) => PgError::source_storage_unavailable(error),
        awr_source::Error::SourceUnavailable(_) => PgError::source_storage_unavailable(
            std::io::Error::other("exact source storage unavailable"),
        ),
        awr_source::Error::SourceConflict(message) => PgError::WritebackRefused(message),
        _ => PgError::Protocol("invalid or unsupported bound source writer".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_replacement_failure_is_classified_without_exposing_the_path() {
        let root = std::env::temp_dir().join(format!("awr-writeback-{}", ulid::Ulid::new()));
        std::fs::create_dir(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        let target = root.join("private-ledger");
        std::fs::write(&target, b"preserved").unwrap();
        let permissions = std::fs::metadata(&target).unwrap().permissions();
        let mut readonly = permissions.clone();
        readonly.set_readonly(true);
        std::fs::set_permissions(&target, readonly).unwrap();
        let guard = LockedSourceFile::open(&root, "private-ledger").unwrap();
        let error = guard
            .replace(&fingerprint(b"preserved"), b"replacement")
            .map_err(source_write_error)
            .unwrap_err();
        // All supported platforms retain bounded storage classification without
        // disclosing the private source path or raw OS message.
        let expected_message = match error.source_storage_reason() {
            Some("permission_denied") => "authoritative source storage permission denied",
            Some("io_error") => "authoritative source storage is unavailable",
            other => panic!("unexpected source-storage classification: {other:?}"),
        };
        assert_eq!(error.to_string(), expected_message);
        assert_eq!(std::fs::read(&target).unwrap(), b"preserved");
        std::fs::set_permissions(&target, permissions).unwrap();
        drop(guard);
        std::fs::remove_dir_all(root).unwrap();
    }
}
