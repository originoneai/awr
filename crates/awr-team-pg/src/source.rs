use crate::error::{PgError, PgResult};
use crate::path::validate_package;
use crate::tx::{bind_scope, bind_workstream_scope, new_id};
use awr_team::{SourceActivationPlan, WorkContract, WorkId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;

#[path = "source_workstreams.rs"]
mod workstreams;
use workstreams::SourceProjection;
pub(crate) use workstreams::require_work_settled;

#[path = "source_planning.rs"]
pub mod planning;
#[path = "source_planning_ops.rs"]
pub mod planning_ops;
#[path = "source_writeback.rs"]
pub mod writeback;

/// Sole authoritative source location bound for Team publish preparation
/// (AWR-TMCP-020). Developers do not need author-laptop files or ledger write
/// access; the server directory or private management repo is the only source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum SoleSourceKind {
    ServerDirectory,
    PrivateManagementRepo,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoleSourceBinding {
    pub kind: SoleSourceKind,
    pub locator: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ledger_relative_path: Option<String>,
}

pub const SOURCE_BINDING_FILE: &str = "source_binding.json";
pub const SOURCE_PROVENANCE_FILE: &str = "source_provenance.json";
pub const WORKSTREAMS_FILE: &str = "workstreams.json";

#[derive(Clone, Debug)]
pub struct SourceFile {
    pub path: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct IngestRequest {
    pub tenant_id: String,
    pub project_id: String,
    pub actor_id: String,
    pub parser_version: String,
    pub files: Vec<SourceFile>,
}

struct PreparedIngest {
    files: Vec<(String, Vec<u8>)>,
    preview_hash: String,
    manifest: Value,
    manifest_digest: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CandidateRecord {
    pub snapshot_id: String,
    pub proposal_id: String,
    pub manifest_digest: String,
    pub parser_version: String,
    pub preview_hash: String,
    pub base_epoch: String,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct CurrentSource {
    pub snapshot_id: String,
    pub manifest_digest: String,
    pub parser_version: String,
    pub authority_epoch: String,
    pub contract_hash: String,
}

/// The aggregate hash identifies the complete source projection. Individual
/// contract hashes retain the V1 codec and are never replaced by this hash.
#[derive(Clone, Debug, serde::Serialize)]
pub struct CurrentWorkstreamSource {
    pub snapshot_id: String,
    pub manifest_digest: String,
    pub parser_version: String,
    pub authority_epoch: String,
    pub projection_hash: String,
    pub contract_hashes: BTreeMap<String, String>,
}

/// Trusted source coordinator API. This is not an authenticated transport;
/// callers must authorize source administration before invoking these methods.
pub struct SourceStore {
    pool: Arc<crate::PgPool>,
}

impl SourceStore {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            pool: Arc::new(crate::PgPool::new(url)),
        }
    }

    /// Build from a validated `tokio_postgres::Config` (see PgPool::from_config).
    pub fn from_config(config: tokio_postgres::Config) -> Self {
        Self {
            pool: Arc::new(crate::PgPool::from_config(config)),
        }
    }

    /// Share the Team read/command pool (HTTP/MCP).
    pub fn from_pool(pool: Arc<crate::PgPool>) -> Self {
        Self { pool }
    }

    async fn connect(&self) -> PgResult<crate::PgClient> {
        self.pool.get().await
    }

    /// Approving a source candidate never grants project membership or action
    /// permissions (AWR-TMCP-020). Member grants remain a separate access path.
    pub const fn publish_approval_grants_member_permissions() -> bool {
        false
    }

    /// Source status, historical human `done`, and old test materials keep
    /// source meaning only. They must not forge PG completion receipts.
    pub const fn source_status_forges_completion_receipts() -> bool {
        false
    }

    /// Validate a first-publish package: require `workstreams.json` plus
    /// `source_binding.json`, preserve immutable digests for full source +
    /// contract + graph, and return the sole source binding.
    pub fn validate_publish_package(files: &[SourceFile]) -> PgResult<SoleSourceBinding> {
        validate_package(
            &files
                .iter()
                .map(|f| (f.path.clone(), f.bytes.clone()))
                .collect::<Vec<_>>(),
        )?;
        let binding_file = files
            .iter()
            .find(|f| f.path == SOURCE_BINDING_FILE)
            .ok_or_else(|| {
                PgError::Protocol(
                    "first publish requires source_binding.json as the sole source location".into(),
                )
            })?;
        let binding: SoleSourceBinding = serde_json::from_slice(&binding_file.bytes)
            .map_err(|e| PgError::Protocol(format!("invalid source_binding.json: {e}")))?;
        if binding.locator.trim().is_empty() {
            return Err(PgError::Protocol("sole source locator required".into()));
        }
        match binding.kind {
            SoleSourceKind::ServerDirectory => {
                if !binding.locator.starts_with('/') {
                    return Err(PgError::Protocol(
                        "server directory sole source must be an absolute path".into(),
                    ));
                }
            }
            SoleSourceKind::PrivateManagementRepo => {
                if !(binding.locator.starts_with("git://")
                    || binding.locator.starts_with("https://")
                    || binding.locator.starts_with("ssh://"))
                {
                    return Err(PgError::Protocol(
                        "private management repo locator must be git://, https://, or ssh://"
                            .into(),
                    ));
                }
            }
        }
        let has_workstreams = files.iter().any(|f| f.path == WORKSTREAMS_FILE);
        if !has_workstreams {
            return Err(PgError::Protocol(
                "first publish requires workstreams.json contract candidates".into(),
            ));
        }
        // Reject inventing membership material inside the source package.
        for file in files {
            if file.path.ends_with("members.json")
                || file.path.ends_with("grants.json")
                || file.path.ends_with("permissions.json")
            {
                return Err(PgError::Protocol(
                    "publish package cannot invent member permissions; approve source does not grant membership".into(),
                ));
            }
        }
        validate_original_source_provenance(files, &binding)?;
        Ok(binding)
    }

    /// First publish uses existing ingest semantics after validating the sole
    /// source binding. Candidate state is separate from activation; callers
    /// must still approve then activate.
    pub async fn ingest_publish_candidate(
        &self,
        request: IngestRequest,
    ) -> PgResult<(CandidateRecord, SoleSourceBinding)> {
        let binding = Self::validate_publish_package(&request.files)?;
        let mut request = request;
        // Embed the binding into the package text already present; ingest stores
        // all files under source_ref so the sole location remains auditable.
        if !request.files.iter().any(|f| f.path == SOURCE_BINDING_FILE) {
            request.files.push(SourceFile {
                path: SOURCE_BINDING_FILE.into(),
                bytes: serde_json::to_vec(&binding)
                    .map_err(|e| PgError::Protocol(e.to_string()))?,
            });
        }
        let candidate = self.ingest(request).await?;
        Ok((candidate, binding))
    }

    pub async fn ingest(&self, request: IngestRequest) -> PgResult<CandidateRecord> {
        // Preserve offline validation and its diagnostics before acquiring PG.
        let prepared = Self::prepare_ingest(&request)?;
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        let candidate = Self::ingest_prepared_in_tx(&tx, request, prepared).await?;
        tx.commit().await?;
        Ok(candidate)
    }

    pub(super) async fn ingest_in_tx(
        tx: &tokio_postgres::Transaction<'_>,
        request: IngestRequest,
    ) -> PgResult<CandidateRecord> {
        let prepared = Self::prepare_ingest(&request)?;
        Self::ingest_prepared_in_tx(tx, request, prepared).await
    }

    fn prepare_ingest(request: &IngestRequest) -> PgResult<PreparedIngest> {
        if request.parser_version.trim().is_empty() {
            return Err(PgError::Protocol("parser_version required".into()));
        }
        let files: Vec<(String, Vec<u8>)> = request
            .files
            .iter()
            .map(|f| (f.path.clone(), f.bytes.clone()))
            .collect();
        validate_package(&files)?;
        let projection = SourceProjection::parse(&files, &request.project_id)?;
        let preview_hash = projection.hash.clone();
        let manifest = build_manifest(&request.parser_version, &files)?;
        let manifest_digest = sha256_hex(
            &serde_json::to_vec(&manifest).map_err(|e| PgError::Protocol(e.to_string()))?,
        );

        Ok(PreparedIngest {
            files,
            preview_hash,
            manifest,
            manifest_digest,
        })
    }

    async fn ingest_prepared_in_tx(
        tx: &tokio_postgres::Transaction<'_>,
        request: IngestRequest,
        prepared: PreparedIngest,
    ) -> PgResult<CandidateRecord> {
        let PreparedIngest {
            files,
            preview_hash,
            manifest,
            manifest_digest,
        } = prepared;

        bind_workstream_scope(tx, &request.tenant_id, &request.project_id).await?;
        crate::tx::lock_active_project(tx, &request.tenant_id, &request.project_id).await?;
        let project = tx
            .query_opt(
                "SELECT authority_epoch FROM awr_team.projects
                 WHERE tenant_id=$1 AND id=$2 FOR SHARE",
                &[&request.tenant_id, &request.project_id],
            )
            .await?;
        let Some(project) = project else {
            return Err(PgError::ProjectNotAvailable);
        };
        let base_epoch: i64 = project.get(0);
        let snapshot_id = new_id();
        let proposal_id = new_id();
        let artifact_id = new_id();
        let sole_source = files
            .iter()
            .find(|(path, _)| path == SOURCE_BINDING_FILE)
            .and_then(|(_, bytes)| serde_json::from_slice::<SoleSourceBinding>(bytes).ok());
        let source_ref = json!({
            "manifest": manifest,
            "sole_source": sole_source,
            "files": files
                .iter()
                .map(|(path, bytes)| {
                    json!({
                        "path": path,
                        "sha256": sha256_hex(bytes),
                        "bytes": bytes.len() as u64,
                        "text": String::from_utf8_lossy(bytes),
                    })
                })
                .collect::<Vec<_>>(),
        });
        tx.execute(
            "INSERT INTO awr_team.artifacts(
                tenant_id, project_id, id, object_key, sha256, byte_length,
                media_type, state, created_by, content)
             VALUES ($1,$2,$3,$4,$5,$6,'application/json','finalized',$7,$8)",
            &[
                &request.tenant_id,
                &request.project_id,
                &artifact_id,
                &format!("snapshots/{snapshot_id}"),
                &manifest_digest,
                &(manifest.to_string().len() as i64),
                &request.actor_id,
                &manifest.to_string().into_bytes(),
            ],
        )
        .await?;
        tx.execute(
            "INSERT INTO awr_team.source_snapshots(
                tenant_id, project_id, id, manifest_digest, source_ref_json,
                artifact_id, parser_version, created_by)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
            &[
                &request.tenant_id,
                &request.project_id,
                &snapshot_id,
                &manifest_digest,
                &source_ref,
                &artifact_id,
                &request.parser_version,
                &request.actor_id,
            ],
        )
        .await?;
        tx.execute(
            "INSERT INTO awr_team.source_proposals(
                tenant_id, project_id, id, base_epoch, candidate_snapshot_id,
                preview_hash, state, reason, author_actor_id)
             VALUES ($1,$2,$3,$4,$5,$6,'pending','ingest',$7)",
            &[
                &request.tenant_id,
                &request.project_id,
                &proposal_id,
                &base_epoch,
                &snapshot_id,
                &preview_hash,
                &request.actor_id,
            ],
        )
        .await?;
        Ok(CandidateRecord {
            snapshot_id,
            proposal_id,
            manifest_digest,
            parser_version: request.parser_version,
            preview_hash,
            base_epoch: base_epoch.to_string(),
        })
    }

    pub async fn approve(
        &self,
        tenant_id: &str,
        project_id: &str,
        proposal_id: &str,
        reviewer_actor_id: &str,
        candidate_digest: &str,
    ) -> PgResult<String> {
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_workstream_scope(&tx, tenant_id, project_id).await?;
        crate::tx::lock_active_project(&tx, tenant_id, project_id).await?;
        let row = tx
            .query_opt(
                "SELECT p.author_actor_id, s.manifest_digest, p.state
                 FROM awr_team.source_proposals p
                 JOIN awr_team.source_snapshots s
                   ON s.tenant_id=p.tenant_id AND s.project_id=p.project_id
                  AND s.id=p.candidate_snapshot_id
                 WHERE p.tenant_id=$1 AND p.project_id=$2 AND p.id=$3
                 FOR UPDATE OF p",
                &[&tenant_id, &project_id, &proposal_id],
            )
            .await?
            .ok_or_else(|| PgError::Protocol("proposal not found".into()))?;
        let author: String = row.get(0);
        let digest: String = row.get(1);
        let state: String = row.get(2);
        if author == reviewer_actor_id {
            return Err(PgError::AuthorCannotApprove);
        }
        validate_reviewer(&tx, tenant_id, project_id, reviewer_actor_id).await?;
        if digest != candidate_digest {
            return Err(PgError::StaleApproval);
        }
        if state != "pending" && state != "approved" {
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
                &reviewer_actor_id,
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

    pub async fn activate(
        &self,
        tenant_id: &str,
        project_id: &str,
        actor_id: &str,
        proposal_id: &str,
        plan: &SourceActivationPlan,
    ) -> PgResult<CurrentSource> {
        let result = self
            .activate_inner(
                tenant_id,
                project_id,
                actor_id,
                proposal_id,
                plan,
                false,
                false,
                None,
            )
            .await?;
        Ok(CurrentSource {
            snapshot_id: result.snapshot_id,
            manifest_digest: result.manifest_digest,
            parser_version: result.parser_version,
            authority_epoch: result.authority_epoch,
            contract_hash: result.projection_hash,
        })
    }

    /// Activate with server-derived impact; a caller gate can only veto the operation.
    pub async fn activate_workstreams_with_impact(
        &self,
        tenant_id: &str,
        project_id: &str,
        actor_id: &str,
        proposal_id: &str,
        plan: &SourceActivationPlan,
        impact: &crate::source::writeback::ActivationImpactGate,
    ) -> PgResult<CurrentWorkstreamSource> {
        self.activate_inner(
            tenant_id,
            project_id,
            actor_id,
            proposal_id,
            plan,
            true,
            false,
            Some(impact),
        )
        .await
    }

    /// Explicit opt-in; never reinterpret `activate`'s singular contract hash.
    pub async fn activate_workstreams(
        &self,
        tenant_id: &str,
        project_id: &str,
        actor_id: &str,
        proposal_id: &str,
        plan: &SourceActivationPlan,
    ) -> PgResult<CurrentWorkstreamSource> {
        self.activate_inner(
            tenant_id,
            project_id,
            actor_id,
            proposal_id,
            plan,
            true,
            false,
            None,
        )
        .await
    }

    #[cfg(feature = "pg-tests")]
    #[doc(hidden)]
    pub async fn abort_workstreams_after_installing_projection(
        &self,
        tenant_id: &str,
        project_id: &str,
        actor_id: &str,
        proposal_id: &str,
        plan: &SourceActivationPlan,
    ) -> PgResult<()> {
        match self
            .activate_inner(
                tenant_id,
                project_id,
                actor_id,
                proposal_id,
                plan,
                true,
                true,
                None,
            )
            .await
        {
            Err(PgError::Protocol(message)) if message == "injected activate abort" => Ok(()),
            other => other.map(|_| ()),
        }
    }

    pub async fn abort_after_installing_projection(
        &self,
        tenant_id: &str,
        project_id: &str,
        actor_id: &str,
        proposal_id: &str,
        plan: &SourceActivationPlan,
    ) -> PgResult<()> {
        match self
            .activate_inner(
                tenant_id,
                project_id,
                actor_id,
                proposal_id,
                plan,
                false,
                true,
                None,
            )
            .await
        {
            Err(PgError::Protocol(message)) if message == "injected activate abort" => Ok(()),
            other => other.map(|_| ()),
        }
    }

    async fn activate_inner(
        &self,
        tenant_id: &str,
        project_id: &str,
        actor_id: &str,
        proposal_id: &str,
        plan: &SourceActivationPlan,
        scoped: bool,
        abort: bool,
        impact: Option<&crate::source::writeback::ActivationImpactGate>,
    ) -> PgResult<CurrentWorkstreamSource> {
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        let current = self
            .activate_in_tx(
                &tx,
                tenant_id,
                project_id,
                actor_id,
                proposal_id,
                plan,
                scoped,
                abort,
                impact,
                None,
            )
            .await?;
        tx.commit().await?;
        Ok(current)
    }

    pub(super) async fn activate_in_tx(
        &self,
        tx: &tokio_postgres::Transaction<'_>,
        tenant_id: &str,
        project_id: &str,
        actor_id: &str,
        proposal_id: &str,
        plan: &SourceActivationPlan,
        scoped: bool,
        abort: bool,
        impact: Option<&crate::source::writeback::ActivationImpactGate>,
        pending_request: Option<&str>,
    ) -> PgResult<CurrentWorkstreamSource> {
        if scoped {
            bind_workstream_scope(tx, tenant_id, project_id).await?;
            // Serialize enablement with every legacy entrypoint before taking
            // the project lock. Ordinary legacy updates share the mode row.
            tx.query_opt(
                "SELECT enabled FROM awr_team.workstream_modes
                WHERE tenant_id=$1 AND project_id=$2 FOR UPDATE",
                &[&tenant_id, &project_id],
            )
            .await?
            .ok_or(PgError::ProjectNotAvailable)?;
        } else {
            bind_scope(tx, tenant_id, project_id).await?;
        }
        crate::tx::lock_active_project(tx, tenant_id, project_id).await?;
        writeback::admission::require_source_available(tx, tenant_id, project_id, pending_request)
            .await?;
        let locked = tx
            .query_opt(
                "SELECT authority_epoch, active_snapshot_id, project_revision
                 FROM awr_team.projects
                 WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
                &[&tenant_id, &project_id],
            )
            .await?
            .ok_or(PgError::ProjectNotAvailable)?;
        let epoch: i64 = locked.get(0);
        let previous_snapshot: Option<String> = locked.get(1);
        let revision: i64 = locked.get(2);
        let expected: i64 = plan
            .expected_authority_epoch
            .parse()
            .map_err(|_| PgError::EpochMismatch)?;
        if expected != epoch {
            return Err(PgError::EpochMismatch);
        }

        let row = tx
            .query_opt(
                "SELECT p.state, p.base_epoch, s.id, s.manifest_digest, s.parser_version,
                        s.source_ref_json
                 FROM awr_team.source_proposals p
                 JOIN awr_team.source_snapshots s
                   ON s.tenant_id=p.tenant_id AND s.project_id=p.project_id
                  AND s.id=p.candidate_snapshot_id
                 WHERE p.tenant_id=$1 AND p.project_id=$2 AND p.id=$3",
                &[&tenant_id, &project_id, &proposal_id],
            )
            .await?
            .ok_or_else(|| PgError::Protocol("proposal not found".into()))?;
        let state: String = row.get(0);
        let base_epoch: i64 = row.get(1);
        let snapshot_id: String = row.get(2);
        let digest: String = row.get(3);
        let parser_version: String = row.get(4);
        let source_ref: Value = row.get(5);
        // The candidate must have been generated from the CURRENT baseline.
        // A caller refreshing expected_authority_epoch after another
        // activation must not push a stale-base candidate over it (CR #37
        // P2-2). Deliberate rollback needs its own explicit operation.
        if base_epoch != epoch {
            return Err(PgError::EpochMismatch);
        }
        if state != "approved" {
            return Err(PgError::CandidateNotApproved);
        }
        if digest != plan.candidate_digest || digest != plan.approved_candidate_digest {
            return Err(PgError::StaleApproval);
        }
        if parser_version != plan.parser_version {
            return Err(PgError::ParserMismatch);
        }
        let approval = tx
            .query_opt(
                "SELECT candidate_digest, reviewer_actor_id FROM awr_team.source_approvals
                 WHERE tenant_id=$1 AND project_id=$2 AND proposal_id=$3
                   AND decision='approve'
                 ORDER BY decided_at DESC LIMIT 1",
                &[&tenant_id, &project_id, &proposal_id],
            )
            .await?
            .ok_or(PgError::CandidateNotApproved)?;
        let approved_digest: String = approval.get(0);
        let approval_reviewer: String = approval.get(1);
        if approved_digest != digest {
            return Err(PgError::StaleApproval);
        }
        // Approvals written before reviewer validation existed (or by any
        // legacy path) must not activate: the consumed approval is checked
        // with the SAME rules as a fresh one (CR #54 P2).
        validate_reviewer(tx, tenant_id, project_id, &approval_reviewer).await?;

        let files = files_from_ref(&source_ref)?;
        validate_package(&files)?;
        let manifest = build_manifest(&parser_version, &files)?;
        if source_ref.get("manifest") != Some(&manifest)
            || sha256_hex(
                &serde_json::to_vec(&manifest).map_err(|e| PgError::Protocol(e.to_string()))?,
            ) != digest
        {
            return Err(PgError::SnapshotDrift("manifest".into()));
        }
        let projection = SourceProjection::parse(&files, project_id)?;
        if projection.bundle.is_some() != scoped {
            return Err(PgError::Unsupported(
                "activation API does not match the source codec".into(),
            ));
        }
        workstreams::reject_external_graph(&files)?;
        let affected = projection
            .validate_transition_with_impact(
                tx,
                tenant_id,
                project_id,
                previous_snapshot.as_deref(),
                impact,
            )
            .await?;
        projection
            .install(tx, tenant_id, project_id, &snapshot_id)
            .await?;
        let work_id = if scoped {
            None
        } else {
            Some(projection.contracts[0].work_id.as_str().to_string())
        };
        if abort {
            return Err(PgError::Protocol("injected activate abort".into()));
        }
        let preparation_invalidations = projection
            .invalidate_affected_preparations(tx, tenant_id, project_id, &affected)
            .await?;
        let next_epoch = epoch + 1;
        let next_revision = revision + 1;
        let completion_invalidations = if scoped {
            projection
                .invalidate_stale_completions(tx, tenant_id, project_id, &snapshot_id)
                .await?
        } else {
            Vec::new()
        };
        tx.execute(
            "UPDATE awr_team.projects
             SET active_snapshot_id=$1, authority_epoch=$2, project_revision=$3
             WHERE tenant_id=$4 AND id=$5 AND authority_epoch=$6",
            &[
                &snapshot_id,
                &next_epoch,
                &next_revision,
                &tenant_id,
                &project_id,
                &epoch,
            ],
        )
        .await?;
        tx.execute(
            "UPDATE awr_team.source_proposals SET state='activated'
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant_id, &project_id, &proposal_id],
        )
        .await?;
        let event_id = new_id();
        let payload = json!({
            "snapshot_id": snapshot_id,
            "previous_snapshot_id": previous_snapshot,
            "parser_version": parser_version,
            "manifest_digest": digest,
            "completion_invalidations": completion_invalidations,
            "activation_impact": preparation_invalidations,
        });
        tx.execute(
            "INSERT INTO awr_team.events(
                tenant_id, project_id, id, project_revision, event_index,
                event_type, actor_id, work_id, payload_json)
             VALUES ($1,$2,$3,$4,0,'source.activated',$5,$6,$7)",
            &[
                &tenant_id,
                &project_id,
                &event_id,
                &next_revision,
                &actor_id,
                &work_id,
                &payload,
            ],
        )
        .await?;
        Ok(CurrentWorkstreamSource {
            snapshot_id,
            manifest_digest: digest,
            parser_version,
            authority_epoch: next_epoch.to_string(),
            projection_hash: projection.hash,
            contract_hashes: projection.hashes,
        })
    }

    pub async fn current(
        &self,
        tenant_id: &str,
        project_id: &str,
        work_id: &str,
    ) -> PgResult<CurrentSource> {
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_scope(&tx, tenant_id, project_id).await?;
        let row = tx
            .query_opt(
                "SELECT p.active_snapshot_id, p.authority_epoch, s.manifest_digest,
                        s.parser_version, c.contract_hash
                 FROM awr_team.projects p
                 JOIN awr_team.source_snapshots s
                   ON s.tenant_id=p.tenant_id AND s.project_id=p.id
                  AND s.id=p.active_snapshot_id
                 JOIN awr_team.work_contracts c
                   ON c.tenant_id=p.tenant_id AND c.project_id=p.id
                  AND c.snapshot_id=p.active_snapshot_id AND c.work_id=$3
                 WHERE p.tenant_id=$1 AND p.id=$2",
                &[&tenant_id, &project_id, &work_id],
            )
            .await?;
        let Some(row) = row else {
            return Err(PgError::InactiveCandidate);
        };
        Ok(CurrentSource {
            snapshot_id: row.get(0),
            authority_epoch: {
                let epoch: i64 = row.get(1);
                epoch.to_string()
            },
            manifest_digest: row.get(2),
            parser_version: row.get(3),
            contract_hash: row.get(4),
        })
    }

    pub async fn contract_for_snapshot(
        &self,
        tenant_id: &str,
        project_id: &str,
        snapshot_id: &str,
        work_id: &str,
    ) -> PgResult<String> {
        let current = self.current(tenant_id, project_id, work_id).await?;
        if current.snapshot_id != snapshot_id {
            return Err(PgError::InactiveCandidate);
        }
        Ok(current.contract_hash)
    }
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn build_manifest(parser_version: &str, files: &[(String, Vec<u8>)]) -> PgResult<Value> {
    Ok(json!({
        "schema_version": 1,
        "parser_version": parser_version,
        "files": files.iter().map(|(path, bytes)| json!({
            "path": path,
            "sha256": sha256_hex(bytes),
            "bytes": bytes.len() as u64,
        })).collect::<Vec<_>>(),
    }))
}

fn parse_contract(files: &[(String, Vec<u8>)]) -> PgResult<WorkContract> {
    let bytes = files
        .iter()
        .find(|(path, _)| path == "contract.json")
        .map(|(_, bytes)| bytes.as_slice())
        .ok_or_else(|| PgError::Protocol("contract.json required".into()))?;
    let mut contract: WorkContract =
        serde_json::from_slice(bytes).map_err(|e| PgError::Protocol(e.to_string()))?;
    if contract.work_id.as_str().is_empty() {
        contract.work_id = WorkId::new("work-a").map_err(|e| PgError::Protocol(e.to_string()))?;
    }
    contract
        .validate()
        .map_err(|e| PgError::Protocol(e.to_string()))?;
    Ok(contract)
}

async fn validate_reviewer(
    tx: &tokio_postgres::Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    reviewer_actor_id: &str,
) -> PgResult<()> {
    crate::tx::validate_reviewer(tx, tenant_id, project_id, reviewer_actor_id).await
}

pub(crate) fn files_from_ref(source_ref: &Value) -> PgResult<Vec<(String, Vec<u8>)>> {
    let files = source_ref
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| PgError::Protocol("snapshot files missing".into()))?;
    files
        .iter()
        .map(|file| {
            let path = file
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| PgError::Protocol("file path missing".into()))?
                .to_string();
            let text = file
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| PgError::Protocol("file text missing".into()))?;
            let bytes = text.as_bytes().to_vec();
            // Fail closed on drift: the recorded digest/length describe the
            // ORIGINAL bytes, so restored content must match them exactly
            // (CR #37 P2-3).
            let expected_sha = file
                .get("sha256")
                .and_then(Value::as_str)
                .ok_or_else(|| PgError::Protocol("file sha256 missing".into()))?;
            let expected_len = file
                .get("bytes")
                .and_then(Value::as_u64)
                .ok_or_else(|| PgError::Protocol("file byte length missing".into()))?;
            if sha256_hex(&bytes) != expected_sha || bytes.len() as u64 != expected_len {
                return Err(PgError::SnapshotDrift(path));
            }
            Ok((path, bytes))
        })
        .collect()
}

fn validate_original_source_provenance(
    files: &[SourceFile],
    binding: &SoleSourceBinding,
) -> PgResult<()> {
    let ledger_path = binding
        .ledger_relative_path
        .as_deref()
        .ok_or_else(|| PgError::Protocol("sole source ledger_relative_path required".into()))?;
    let ledger = files
        .iter()
        .find(|f| f.path == ledger_path)
        .ok_or_else(|| {
            PgError::Protocol(format!(
                "publish package missing original ledger bytes at {ledger_path}"
            ))
        })?;
    let provenance = files
        .iter()
        .find(|f| f.path == SOURCE_PROVENANCE_FILE)
        .ok_or_else(|| {
            PgError::Protocol("publish package missing source_provenance.json".into())
        })?;
    let meta: serde_json::Value = serde_json::from_slice(&provenance.bytes)
        .map_err(|e| PgError::Protocol(format!("invalid source_provenance.json: {e}")))?;
    let declared = meta
        .get("source_version_digest")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            PgError::Protocol("source_provenance.json missing source_version_digest".into())
        })?;
    let actual = format!("sha256:{}", sha256_hex(&ledger.bytes));
    if declared != actual {
        return Err(PgError::Protocol(
            "source_version_digest does not match original ledger bytes".into(),
        ));
    }
    if meta.get("ledger_relative_path").and_then(|v| v.as_str()) != Some(ledger_path) {
        return Err(PgError::Protocol(
            "source_provenance.json ledger_relative_path mismatch".into(),
        ));
    }
    if meta.get("source_status_notes").is_none() {
        return Err(PgError::Protocol(
            "source_provenance.json missing source_status_notes".into(),
        ));
    }
    let _ = binding;
    Ok(())
}
#[cfg(test)]
mod publish_prep_tests {
    use super::*;

    fn binding() -> SoleSourceBinding {
        SoleSourceBinding {
            kind: SoleSourceKind::ServerDirectory,
            locator: "/var/awr/team/demo".into(),
            ledger_relative_path: Some("ledger.yaml".into()),
        }
    }

    fn provenance_and_ledger(binding: &SoleSourceBinding) -> (SourceFile, SourceFile) {
        let ledger_path = binding.ledger_relative_path.clone().unwrap();
        let ledger_bytes = b"# original ledger revision\nworkstreams: {}\n".to_vec();
        let digest = format!("sha256:{}", super::sha256_hex(&ledger_bytes));
        let provenance = serde_json::json!({
            "source_version_digest": digest,
            "ledger_relative_path": ledger_path,
            "source_status_notes": []
        });
        (
            SourceFile {
                path: ledger_path,
                bytes: ledger_bytes,
            },
            SourceFile {
                path: SOURCE_PROVENANCE_FILE.into(),
                bytes: serde_json::to_vec(&provenance).unwrap(),
            },
        )
    }

    fn workstreams_bytes() -> Vec<u8> {
        // Minimal structurally valid marker; projection parse is covered by PG tests.
        br#"{"codec":"awr-team-workstreams-v1","catalog":{"version":1,"project_id":"demo","legacy_default":null,"workstreams":[]},"contracts":[]}"#.to_vec()
    }

    #[test]
    fn approve_source_does_not_grant_member_permissions() {
        assert!(!SourceStore::publish_approval_grants_member_permissions());
    }

    #[test]
    fn source_status_does_not_forge_completion_receipts() {
        assert!(!SourceStore::source_status_forges_completion_receipts());
        assert!(!crate::source::workstreams::source_status_is_completion_proof());
    }

    #[test]
    fn publish_package_requires_sole_source_binding_and_workstreams() {
        let err = SourceStore::validate_publish_package(&[SourceFile {
            path: WORKSTREAMS_FILE.into(),
            bytes: workstreams_bytes(),
        }])
        .unwrap_err();
        assert!(err.to_string().contains("source_binding.json"), "{err}");

        let err = SourceStore::validate_publish_package(&[SourceFile {
            path: SOURCE_BINDING_FILE.into(),
            bytes: serde_json::to_vec(&binding()).unwrap(),
        }])
        .unwrap_err();
        assert!(err.to_string().contains("workstreams.json"), "{err}");
    }

    #[test]
    fn publish_package_rejects_invented_membership_files() {
        let b = binding();
        let (ledger, provenance) = provenance_and_ledger(&b);
        let err = SourceStore::validate_publish_package(&[
            SourceFile {
                path: WORKSTREAMS_FILE.into(),
                bytes: workstreams_bytes(),
            },
            SourceFile {
                path: SOURCE_BINDING_FILE.into(),
                bytes: serde_json::to_vec(&b).unwrap(),
            },
            ledger,
            provenance,
            SourceFile {
                path: "grants.json".into(),
                bytes: b"[]".to_vec(),
            },
        ])
        .unwrap_err();
        assert!(err.to_string().contains("member permissions"), "{err}");
    }

    #[test]
    fn happy_path_publish_package_binds_sole_source() {
        let b = binding();
        let (ledger, provenance) = provenance_and_ledger(&b);
        let binding = SourceStore::validate_publish_package(&[
            SourceFile {
                path: WORKSTREAMS_FILE.into(),
                bytes: workstreams_bytes(),
            },
            SourceFile {
                path: SOURCE_BINDING_FILE.into(),
                bytes: serde_json::to_vec(&b).unwrap(),
            },
            ledger,
            provenance,
        ])
        .unwrap();
        assert_eq!(binding.locator, "/var/awr/team/demo");
    }

    #[test]
    fn publish_package_rejects_digest_mismatch_for_original_ledger() {
        let b = binding();
        let (mut ledger, provenance) = provenance_and_ledger(&b);
        ledger.bytes = b"# tampered\n".to_vec();
        let err = SourceStore::validate_publish_package(&[
            SourceFile {
                path: WORKSTREAMS_FILE.into(),
                bytes: workstreams_bytes(),
            },
            SourceFile {
                path: SOURCE_BINDING_FILE.into(),
                bytes: serde_json::to_vec(&b).unwrap(),
            },
            ledger,
            provenance,
        ])
        .unwrap_err();
        assert!(err.to_string().contains("source_version_digest"), "{err}");
    }
}
