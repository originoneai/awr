//! Authoritative planning writeback and consistent activation (AWR-TMCP-022).
//!
//! Consumes TMCP-021 publish receipts (`source_writeback_pending`), writes
//! approved changes into the bound sole source under WS-022 fingerprint/
//! recovery semantics, and activates one coherent PG snapshot. Impact gating
//! reuses WS-032 selective replan rules and does not drop the project-wide
//! active-claim barrier unless impact is proven.
use super::{IngestRequest, PgError, PgResult, SourceFile, SourceStore};
use crate::lock_order::lock_works_sorted;
use crate::tx::{bind_workstream_scope, lock_active_project, new_id};
use crate::workstream_auth::{authenticate, authenticate_writer};
#[allow(unused_imports)]
use awr_source::SOURCE_BINDING_FILE;
use awr_source::{
    PublishPrepOptions, SoleSourceLocation, apply_planning_changes_to_ledger, fingerprint,
    prepare_publish_from_ledger_bytes, refuse_external_overwrite,
    refuse_runtime_field_in_source_write, source_status_notes_are_completion_receipts,
};
use awr_team::{DraftChange, SourceActivationPlan};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use tokio_postgres::Transaction;

/// Proven impact set supplied to activation. When `impact_proven` is false the
/// historical project-wide claim barrier remains in force.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationImpactGate {
    pub impact_proven: bool,
    pub allow_activation: bool,
    pub affected_work_ids: Vec<String>,
    pub unrelated_work_ids: Vec<String>,
    /// Works with an explicit stop/reconcile recorded for this request.
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
    /// When impact cannot be proven, leave false — activation conservatively refuses.
    pub impact_proven: bool,
    /// Works for which an explicit external-process stop/reconcile was recorded.
    #[serde(default)]
    pub stopped_work_ids: Vec<String>,
}

impl SourceStore {
    /// Activate a published planning candidate: write authoritative source bytes
    /// then commit a consistent PG snapshot. Same `request_id` replays to one
    /// effective activation.
    pub async fn activate_planning_writeback(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        req: &WritebackActivateRequest,
    ) -> PgResult<Value> {
        if req.request_id.trim().is_empty() || req.request_id.len() > 200 {
            return Err(PgError::Protocol(
                "request_id required for idempotent writeback".into(),
            ));
        }
        assert!(!source_status_notes_are_completion_receipts());

        let mut client = self.connect().await?;
        crate::check_schema(&client).await?;

        // Idempotent replay under a short read.
        if let Some(existing) = self
            .get_planning_activation_receipt(tenant_id, project_id, bearer, &req.request_id)
            .await?
        {
            return Ok(json!({
                "already_recorded": true,
                "receipt": existing,
            }));
        }

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

        // Lock order (WS-023): project barrier → sorted affected works → receipts.
        bind_workstream_scope(&tx, tenant_id, project_id).await?;
        lock_active_project(&tx, tenant_id, project_id).await?;

        let publish = tx
            .query_opt(
                "SELECT id, candidate_id, candidate_digest, draft_revision, approval_id,
                        publisher_actor_id, source_writeback_pending, activation_receipt_id
                 FROM awr_team.planning_publish_receipts
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3
                 FOR UPDATE",
                &[&tenant_id, &project_id, &req.publish_receipt_id],
            )
            .await?
            .ok_or_else(|| PgError::Protocol("publish receipt not found".into()))?;
        let candidate_id: String = publish.get(1);
        let candidate_digest: String = publish.get(2);
        let approval_id: String = publish.get(4);
        let publisher_actor_id: String = publish.get(5);
        let pending: bool = publish.get(6);
        let existing_activation: Option<String> = publish.get(7);
        if let Some(id) = existing_activation {
            let receipt = load_activation_receipt(&tx, tenant_id, project_id, &id).await?;
            tx.commit().await?;
            return Ok(json!({"already_recorded": true, "receipt": receipt}));
        }
        if !pending {
            return Err(PgError::Protocol(
                "publish receipt is not pending source writeback".into(),
            ));
        }

        let approval = tx
            .query_one(
                "SELECT approver_actor_id, candidate_digest FROM awr_team.planning_approvals
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant_id, &project_id, &approval_id],
            )
            .await?;
        let approver_actor_id: String = approval.get(0);
        let approved_digest: String = approval.get(1);
        if approved_digest != candidate_digest {
            return Err(PgError::StaleApproval);
        }

        let changes = load_published_changes(&tx, tenant_id, project_id, &candidate_id).await?;
        // Source vs runtime field authority is enforced by TaskDraft schema and
        // awr-source::planning_writeback; runtime-only keys never appear here.
        debug_assert!(refuse_runtime_field_in_source_write("claim_id").is_err());

        let all_works = load_all_work_ids(&tx, tenant_id, project_id).await?;
        let mut affected: BTreeSet<String> = BTreeSet::new();
        for change in &changes {
            affected.insert(change.after.work_id.clone());
            if let Some(before) = &change.before {
                affected.insert(before.work_id.clone());
            }
            for child in &change.after.split_children {
                affected.insert(child.clone());
            }
        }
        // Recheck read-sets: works that observed an affected work are affected.
        for work_id in &all_works {
            let rs = load_read_set_work_ids(&tx, tenant_id, project_id, work_id).await?;
            if rs.iter().any(|w| affected.contains(w)) {
                affected.insert(work_id.clone());
            }
        }
        let unrelated: Vec<String> = all_works
            .iter()
            .filter(|w| !affected.contains(*w))
            .cloned()
            .collect();
        let affected_vec: Vec<String> = affected.iter().cloned().collect();

        lock_works_sorted(&tx, tenant_id, project_id, "main", &affected_vec).await?;

        let gate = build_impact_gate(
            &tx,
            tenant_id,
            project_id,
            &affected_vec,
            &unrelated,
            &req.stopped_work_ids,
            req.impact_proven,
        )
        .await?;

        if !gate.allow_activation {
            let journal_id_body = json!({
                "gate": gate,
                "candidate_id": candidate_id,
                "candidate_digest": candidate_digest,
            });
            upsert_journal(
                &tx,
                tenant_id,
                project_id,
                &req.request_id,
                &candidate_id,
                &candidate_digest,
                &req.publish_receipt_id,
                "refused",
                "",
                "",
                &publisher_actor_id,
                Some(&approver_actor_id),
                &affected_vec,
                &unrelated,
                &gate,
                &journal_id_body,
            )
            .await?;
            tx.commit().await?;
            return Err(if !gate.impact_proven {
                PgError::ActivationImpactUnproven(
                    gate.refuse_reason
                        .unwrap_or_else(|| "impact unproven".into()),
                )
            } else {
                PgError::WritebackRefused(
                    gate.refuse_reason
                        .unwrap_or_else(|| "writeback refused".into()),
                )
            });
        }

        // --- Build + validate the complete candidate BEFORE any source mutation.
        // `validated` is committed before the file write, so a crash after the
        // write and before `source_written` must not apply the patch again.
        // Resume source_written / pg_activating / validated without re-applying
        // CreateTask. ---
        let ledger_path = req.source_root.join(&req.ledger_relative_path);
        let location =
            SoleSourceLocation::server_directory(&req.source_root, &req.ledger_relative_path)
                .map_err(|e| PgError::Protocol(e.to_string()))?;

        let prior = load_journal_row(&tx, tenant_id, project_id, &req.request_id).await?;
        let prior_phase = prior.as_ref().map(|j| j.phase.as_str()).unwrap_or("");

        let (before_fingerprint, after_fingerprint, after_bytes, package, source_already_written) =
            if matches!(
                prior_phase,
                "validated" | "source_written" | "pg_activating"
            ) {
                let journal = prior.expect("phase implies journal row");
                let disk =
                    std::fs::read(&ledger_path).map_err(PgError::source_storage_unavailable)?;
                let disk_fp = fingerprint(&disk);
                if disk_fp == journal.after_fingerprint {
                    // Source write landed; resume activation without re-applying creates.
                    let package = prepare_publish_from_ledger_bytes(
                        &location,
                        &req.source_root,
                        &disk,
                        project_id,
                        &PublishPrepOptions::default(),
                    )
                    .map_err(|e| PgError::Protocol(e.to_string()))?;
                    (
                        journal.before_fingerprint,
                        journal.after_fingerprint,
                        disk,
                        package,
                        true,
                    )
                } else if disk_fp == journal.before_fingerprint {
                    // Write never persisted; rebuild, validate, then write below.
                    let patch = apply_planning_changes_to_ledger(&disk, &changes)
                        .map_err(|e| PgError::Protocol(e.to_string()))?;
                    if patch.after_fingerprint != journal.after_fingerprint {
                        return Err(PgError::Protocol(
                            "resume rebuild fingerprint diverged from journal intent".into(),
                        ));
                    }
                    let package = prepare_publish_from_ledger_bytes(
                        &location,
                        &req.source_root,
                        &patch.after_bytes,
                        project_id,
                        &PublishPrepOptions::default(),
                    )
                    .map_err(|e| PgError::Protocol(e.to_string()))?;
                    (
                        patch.before_fingerprint,
                        patch.after_fingerprint,
                        patch.after_bytes,
                        package,
                        false,
                    )
                } else {
                    return Err(PgError::Protocol(
                        "authoritative source changed externally; refusing overwrite of others' work"
                            .into(),
                    ));
                }
            } else {
                // Fresh / planned / refused-retry: plan patch and validate fully
                // before the first authoritative source mutation.
                let before_bytes =
                    std::fs::read(&ledger_path).map_err(PgError::source_storage_unavailable)?;
                let observed_fp = fingerprint(&before_bytes);
                let patch = apply_planning_changes_to_ledger(&before_bytes, &changes)
                    .map_err(|e| PgError::Protocol(e.to_string()))?;
                refuse_external_overwrite(&patch.before_fingerprint, &observed_fp)
                    .map_err(|e| PgError::Protocol(e.to_string()))?;

                let package = prepare_publish_from_ledger_bytes(
                    &location,
                    &req.source_root,
                    &patch.after_bytes,
                    project_id,
                    &PublishPrepOptions::default(),
                )
                .map_err(|e| PgError::Protocol(e.to_string()))?;
                let files_preview: Vec<SourceFile> = package
                    .files
                    .iter()
                    .map(|f| SourceFile {
                        path: f.path.clone(),
                        bytes: f.bytes.clone(),
                    })
                    .collect();
                let _binding =
                    SourceStore::validate_publish_package(&files_preview).map_err(|e| e)?;

                upsert_journal(
                    &tx,
                    tenant_id,
                    project_id,
                    &req.request_id,
                    &candidate_id,
                    &candidate_digest,
                    &req.publish_receipt_id,
                    "validated",
                    &patch.before_fingerprint,
                    &patch.after_fingerprint,
                    &publisher_actor_id,
                    Some(&approver_actor_id),
                    &affected_vec,
                    &unrelated,
                    &gate,
                    &json!({
                        "phase": "validated",
                        "bundle_digest": package.bundle_digest,
                    }),
                )
                .await?;

                (
                    patch.before_fingerprint,
                    patch.after_fingerprint,
                    patch.after_bytes,
                    package,
                    false,
                )
            };

        // Validate package files once more for the resume path that skipped preview.
        let files: Vec<SourceFile> = package
            .files
            .iter()
            .map(|f| SourceFile {
                path: f.path.clone(),
                bytes: f.bytes.clone(),
            })
            .collect();
        let _binding = SourceStore::validate_publish_package(&files)?;

        // Release the SQL transaction before filesystem write; re-lock after.
        tx.commit().await?;

        if !source_already_written {
            // Fingerprint re-check immediately before write (external race).
            let recheck =
                std::fs::read(&ledger_path).map_err(PgError::source_storage_unavailable)?;
            refuse_external_overwrite(&before_fingerprint, &fingerprint(&recheck))
                .map_err(|e| PgError::Protocol(e.to_string()))?;
            atomic_write(&ledger_path, &after_bytes)?;
        }

        // Re-enter PG for activation.
        let mut client = self.connect().await?;
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
        bind_workstream_scope(&tx, tenant_id, project_id).await?;
        lock_active_project(&tx, tenant_id, project_id).await?;
        lock_works_sorted(&tx, tenant_id, project_id, "main", &affected_vec).await?;

        // Another request may have completed meanwhile.
        if let Some(row) = tx
            .query_opt(
                "SELECT id FROM awr_team.planning_activation_receipts
                 WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
                &[&tenant_id, &project_id, &req.request_id],
            )
            .await?
        {
            let id: String = row.get(0);
            let receipt = load_activation_receipt(&tx, tenant_id, project_id, &id).await?;
            tx.commit().await?;
            return Ok(json!({"already_recorded": true, "receipt": receipt}));
        }

        upsert_journal(
            &tx,
            tenant_id,
            project_id,
            &req.request_id,
            &candidate_id,
            &candidate_digest,
            &req.publish_receipt_id,
            "source_written",
            &before_fingerprint,
            &after_fingerprint,
            &publisher_actor_id,
            Some(&approver_actor_id),
            &affected_vec,
            &unrelated,
            &gate,
            &json!({"phase":"source_written"}),
        )
        .await?;

        upsert_journal(
            &tx,
            tenant_id,
            project_id,
            &req.request_id,
            &candidate_id,
            &candidate_digest,
            &req.publish_receipt_id,
            "pg_activating",
            &before_fingerprint,
            &after_fingerprint,
            &publisher_actor_id,
            Some(&approver_actor_id),
            &affected_vec,
            &unrelated,
            &gate,
            &json!({"phase":"pg_activating", "bundle_digest": package.bundle_digest}),
        )
        .await?;

        // Ingest + approve + activate via helpers that open their own txs.
        drop(tx);
        let (candidate, _binding) = self
            .ingest_publish_candidate(IngestRequest {
                tenant_id: tenant_id.into(),
                project_id: project_id.into(),
                actor_id: publisher_actor_id.clone(),
                parser_version: package.parser_version.clone(),
                files,
            })
            .await
            .map_err(|e| e)?;
        // Bind source approval to the already-verified planning approval.
        // Self-approved ordinary planning is allowed under project policy; do not
        // re-impose author!=reviewer for the derived source proposal.
        self.record_writeback_source_approval(
            tenant_id,
            project_id,
            &candidate.proposal_id,
            &candidate.manifest_digest,
            &approver_actor_id,
            &approval_id,
        )
        .await
        .map_err(|e| e)?;
        let current = self
            .activate_workstreams_with_impact(
                tenant_id,
                project_id,
                &publisher_actor_id,
                &candidate.proposal_id,
                &SourceActivationPlan {
                    candidate_digest: candidate.manifest_digest.clone(),
                    parser_version: candidate.parser_version.clone(),
                    expected_authority_epoch: candidate.base_epoch.clone(),
                    approved_candidate_digest: candidate.manifest_digest.clone(),
                },
                &gate,
            )
            .await
            .map_err(|e| e)?;
        // Finalize receipts.
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_workstream_scope(&tx, tenant_id, project_id).await?;
        lock_active_project(&tx, tenant_id, project_id).await?;
        let activation_id = new_id();
        let audit = json!({
            "request_id": req.request_id,
            "publish_receipt_id": req.publish_receipt_id,
            "candidate_id": candidate_id,
            "candidate_digest": candidate_digest,
            "approval_id": approval_id,
            "approver_actor_id": approver_actor_id,
            "publisher_actor_id": publisher_actor_id,
            "source_version": package.source_version_digest,
            "activated_snapshot_id": current.snapshot_id,
            "authority_epoch": current.authority_epoch,
            "affected_work_ids": affected_vec,
            "unrelated_work_ids": unrelated,
            "before_fingerprint": before_fingerprint,
            "after_fingerprint": after_fingerprint,
            "recovery_actions": gate.recovery_actions,
        });
        tx.execute(
            "INSERT INTO awr_team.planning_activation_receipts(
                tenant_id, project_id, id, request_id, candidate_id, candidate_digest,
                publish_receipt_id, approval_id, approver_actor_id, publisher_actor_id,
                source_version, activated_snapshot_id, authority_epoch,
                before_fingerprint, after_fingerprint, affected_work_ids, unrelated_work_ids,
                audit_json)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16::jsonb,$17::jsonb,$18::jsonb)",
            &[
                &tenant_id,
                &project_id,
                &activation_id,
                &req.request_id,
                &candidate_id,
                &candidate_digest,
                &req.publish_receipt_id,
                &approval_id,
                &approver_actor_id,
                &publisher_actor_id,
                &package.source_version_digest,
                &current.snapshot_id,
                &current.authority_epoch,
                &before_fingerprint,
                &after_fingerprint,
                &json!(affected_vec),
                &json!(unrelated),
                &audit,
            ],
        )
        .await?;
        tx.execute(
            "UPDATE awr_team.planning_publish_receipts
             SET source_writeback_pending=false,
                 activation_receipt_id=$4,
                 source_version=$5,
                 activated_snapshot_id=$6
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[
                &tenant_id,
                &project_id,
                &req.publish_receipt_id,
                &activation_id,
                &package.source_version_digest,
                &current.snapshot_id,
            ],
        )
        .await?;
        upsert_journal(
            &tx,
            tenant_id,
            project_id,
            &req.request_id,
            &candidate_id,
            &candidate_digest,
            &req.publish_receipt_id,
            "completed",
            &before_fingerprint,
            &after_fingerprint,
            &publisher_actor_id,
            Some(&approver_actor_id),
            &affected_vec,
            &unrelated,
            &gate,
            &audit,
        )
        .await?;
        // Mark journal audit_receipt_id
        tx.execute(
            "UPDATE awr_team.planning_writeback_journals
             SET audit_receipt_id=$4, source_version=$5, activated_snapshot_id=$6,
                 authority_epoch=$7, updated_at=clock_timestamp()
             WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
            &[
                &tenant_id,
                &project_id,
                &req.request_id,
                &activation_id,
                &package.source_version_digest,
                &current.snapshot_id,
                &current.authority_epoch,
            ],
        )
        .await?;
        tx.commit().await?;

        Ok(json!({
            "already_recorded": false,
            "receipt_id": activation_id,
            "request_id": req.request_id,
            "candidate_id": candidate_id,
            "candidate_digest": candidate_digest,
            "approver_actor_id": approver_actor_id,
            "source_version": package.source_version_digest,
            "activated_snapshot_id": current.snapshot_id,
            "authority_epoch": current.authority_epoch,
            "before_fingerprint": before_fingerprint,
            "after_fingerprint": after_fingerprint,
            "affected_work_ids": affected_vec,
            "unrelated_work_ids": unrelated,
            "source_bytes_written": true,
            "source_writeback_pending": false,
            "audit": audit,
        }))
    }

    async fn record_writeback_source_approval(
        &self,
        tenant_id: &str,
        project_id: &str,
        proposal_id: &str,
        candidate_digest: &str,
        approver_actor_id: &str,
        planning_approval_id: &str,
    ) -> PgResult<String> {
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        crate::tx::bind_workstream_scope(&tx, tenant_id, project_id).await?;
        lock_active_project(&tx, tenant_id, project_id).await?;
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
        tx.commit().await?;
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

    /// Capability probe for TMCP-022 writeback/activation.
    pub fn planning_writeback_capabilities() -> Value {
        json!({
            "source_writeback": "tmcp_022",
            "precise_patch_fingerprint_recovery": true,
            "barrier_lock_order": "ws023",
            "selective_replan": "ws032",
            "project_claim_barrier_retained_when_unproven": true,
            "cancel_expiry_session_end_prove_process_stopped": false,
            "runtime_fields_writable_via_source": false,
            "source_status_is_completion_receipt": false,
            "idempotent_request_id": true,
            "queryable_activation_receipt": true
        })
    }
}

async fn load_published_changes(
    tx: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    candidate_id: &str,
) -> PgResult<Vec<DraftChange>> {
    let row = tx
        .query_opt(
            "SELECT changes_json, state FROM awr_team.planning_candidates
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant_id, &project_id, &candidate_id],
        )
        .await?
        .ok_or_else(|| PgError::Protocol("planning candidate not found".into()))?;
    let state: String = row.get(1);
    if state != "published" {
        return Err(PgError::Protocol(
            "writeback requires a published planning candidate".into(),
        ));
    }
    serde_json::from_value(row.get(0)).map_err(|e| PgError::Protocol(e.to_string()))
}

async fn load_all_work_ids(
    tx: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
) -> PgResult<Vec<String>> {
    let rows = tx
        .query(
            "SELECT id FROM awr_team.work_items
             WHERE tenant_id=$1 AND project_id=$2 ORDER BY id",
            &[&tenant_id, &project_id],
        )
        .await?;
    Ok(rows.into_iter().map(|r| r.get(0)).collect())
}

async fn load_read_set_work_ids(
    tx: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    work_id: &str,
) -> PgResult<Vec<String>> {
    // Optional table from WS-020; tolerate absence by returning empty (impact
    // then relies on explicit candidate diffs only).
    let exists: bool = tx
        .query_one(
            "SELECT EXISTS(
                SELECT 1 FROM information_schema.tables
                WHERE table_schema='awr_team' AND table_name='operation_readsets')",
            &[],
        )
        .await?
        .get(0);
    if !exists {
        return Ok(Vec::new());
    }
    let rows = tx
        .query(
            "SELECT observed_work_id FROM awr_team.operation_readsets
             WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3",
            &[&tenant_id, &project_id, &work_id],
        )
        .await
        .unwrap_or_default();
    Ok(rows.into_iter().map(|r| r.get(0)).collect())
}

async fn build_impact_gate(
    tx: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    affected: &[String],
    unrelated: &[String],
    stopped: &[String],
    impact_proven: bool,
) -> PgResult<ActivationImpactGate> {
    let stopped_set: BTreeSet<_> = stopped.iter().cloned().collect();
    let mut recovery = vec![
        "recompute affected set from planning candidate diffs".into(),
        "recheck operation read-sets for live claims and nonterminal executions".into(),
        "explicitly stop/reconcile/replan affected works before retry".into(),
    ];
    if !impact_proven {
        return Ok(ActivationImpactGate {
            impact_proven: false,
            allow_activation: false,
            affected_work_ids: affected.to_vec(),
            unrelated_work_ids: unrelated.to_vec(),
            stopped_work_ids: stopped.to_vec(),
            refuse_reason: Some(
                "activation impact cannot be proven; retaining project-wide active-claim barrier"
                    .into(),
            ),
            recovery_actions: recovery,
        });
    }

    let mut allow = true;
    let mut refuse = None;
    for work_id in affected {
        let active_claim: i64 = tx
            .query_one(
                "SELECT count(*) FROM awr_team.claims
                 WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3
                   AND state='active' AND expires_at > clock_timestamp()",
                &[&tenant_id, &project_id, &work_id],
            )
            .await?
            .get(0);
        // Cancelled/expired claims do not prove process stop — only explicit stopped set does.
        let nonterminal: i64 = tx
            .query_one(
                "SELECT count(*) FROM awr_team.executions
                 WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3
                   AND state NOT IN ('succeeded','failed','cancelled')",
                &[&tenant_id, &project_id, &work_id],
            )
            .await?
            .get(0);
        let live = active_claim > 0 || nonterminal > 0;
        if live && !stopped_set.contains(work_id) {
            allow = false;
            refuse = Some(format!(
                "affected work {work_id} requires explicit stop/reconcile/replan (cancel/expiry/end-session do not prove process stopped)"
            ));
            recovery.push(format!("stop and reconcile work {work_id}"));
        }
    }

    Ok(ActivationImpactGate {
        impact_proven: true,
        allow_activation: allow,
        affected_work_ids: affected.to_vec(),
        unrelated_work_ids: unrelated.to_vec(),
        stopped_work_ids: stopped.to_vec(),
        refuse_reason: refuse,
        recovery_actions: recovery,
    })
}

#[derive(Clone, Debug)]
struct JournalRow {
    phase: String,
    before_fingerprint: String,
    after_fingerprint: String,
}

async fn load_journal_row(
    tx: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    request_id: &str,
) -> PgResult<Option<JournalRow>> {
    let row = tx
        .query_opt(
            "SELECT phase, before_fingerprint, after_fingerprint
             FROM awr_team.planning_writeback_journals
             WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
            &[&tenant_id, &project_id, &request_id],
        )
        .await?;
    Ok(row.map(|r| JournalRow {
        phase: r.get(0),
        before_fingerprint: r.get(1),
        after_fingerprint: r.get(2),
    }))
}

async fn upsert_journal(
    tx: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    request_id: &str,
    candidate_id: &str,
    candidate_digest: &str,
    publish_receipt_id: &str,
    phase: &str,
    before_fp: &str,
    after_fp: &str,
    publisher_actor_id: &str,
    approver_actor_id: Option<&str>,
    affected: &[String],
    unrelated: &[String],
    gate: &ActivationImpactGate,
    body: &Value,
) -> PgResult<()> {
    tx.execute(
        "INSERT INTO awr_team.planning_writeback_journals(
            tenant_id, project_id, request_id, candidate_id, candidate_digest,
            publish_receipt_id, phase, before_fingerprint, after_fingerprint,
            approver_actor_id, publisher_actor_id, affected_work_ids, unrelated_work_ids,
            recovery_actions, refuse_reason, body_json)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12::jsonb,$13::jsonb,$14::jsonb,$15,$16::jsonb)
         ON CONFLICT (tenant_id, project_id, request_id) DO UPDATE SET
            phase=EXCLUDED.phase,
            before_fingerprint=EXCLUDED.before_fingerprint,
            after_fingerprint=EXCLUDED.after_fingerprint,
            approver_actor_id=EXCLUDED.approver_actor_id,
            affected_work_ids=EXCLUDED.affected_work_ids,
            unrelated_work_ids=EXCLUDED.unrelated_work_ids,
            recovery_actions=EXCLUDED.recovery_actions,
            refuse_reason=EXCLUDED.refuse_reason,
            body_json=EXCLUDED.body_json,
            updated_at=clock_timestamp()",
        &[
            &tenant_id,
            &project_id,
            &request_id,
            &candidate_id,
            &candidate_digest,
            &publish_receipt_id,
            &phase,
            &before_fp,
            &after_fp,
            &approver_actor_id,
            &publisher_actor_id,
            &json!(affected),
            &json!(unrelated),
            &json!(gate.recovery_actions),
            &gate.refuse_reason,
            &body,
        ],
    )
    .await?;
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

fn atomic_write(path: &Path, bytes: &[u8]) -> PgResult<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = parent.join(format!(
        ".{}.tmcp022.tmp",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("ledger")
    ));
    std::fs::write(&tmp, bytes).map_err(PgError::source_storage_unavailable)?;
    std::fs::rename(&tmp, path).map_err(PgError::source_storage_unavailable)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_replacement_failure_is_classified_without_exposing_the_path() {
        let root = std::env::temp_dir().join(format!("awr-writeback-{}", ulid::Ulid::new()));
        std::fs::create_dir(&root).unwrap();
        let target = root.join("private-ledger");
        // A file cannot replace a nonempty directory, on any supported platform.
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("existing"), b"preserved").unwrap();
        let error = atomic_write(&target, b"replacement").unwrap_err();
        // Windows reports this failed replacement as PermissionDenied, while
        // Unix typically reports a different I/O error. Both must retain the
        // bounded classification and omit private paths and raw OS messages.
        let expected_message = match error.source_storage_reason() {
            Some("permission_denied") => "authoritative source storage permission denied",
            Some("io_error") => "authoritative source storage is unavailable",
            other => panic!("unexpected source-storage classification: {other:?}"),
        };
        assert_eq!(error.to_string(), expected_message);
        assert_eq!(
            std::fs::read(target.join("existing")).unwrap(),
            b"preserved"
        );
        // A failed rename may leave a temporary file: diagnostics must not claim
        // that there were no effects or tell callers to use a new request ID.
        assert_eq!(
            std::fs::read(root.join(".private-ledger.tmcp022.tmp")).unwrap(),
            b"replacement"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
