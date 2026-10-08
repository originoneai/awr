//! Exact original intent, identity, publication, baseline and source provenance.
use super::*;
use crate::source::{CandidateRecord, SoleSourceBinding};
use crate::workstream_auth::ReaderAuthority;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Publication {
    pub candidate_id: String,
    pub candidate_digest: String,
    pub draft_revision: i32,
    pub approval_id: String,
    pub approver_actor_id: String,
    pub publisher_actor_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Intent {
    pub codec: String,
    pub tenant_id: String,
    pub project_id: String,
    pub request: WritebackActivateRequest,
    pub planning_request: Option<Value>,
    pub planning_request_hash: Option<String>,
    pub actor_id: String,
    pub client_id: String,
    pub coordinator_epoch: String,
    pub publication: Publication,
    pub base_snapshot_id: String,
    pub base_authority_epoch: String,
    pub registered_source: Option<SoleSourceBinding>,
    pub before_fingerprint: String,
    pub after_fingerprint: String,
    pub source_version: String,
    pub candidate: Option<CandidateRecord>,
    pub affected_work_ids: Vec<String>,
    pub dependency_work_ids: Vec<String>,
}

pub(super) async fn command_origin(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    req: &WritebackActivateRequest,
    publication: &Publication,
) -> PgResult<(Option<Value>, Option<String>)> {
    let row = tx.query_opt(
        "SELECT op,request_hash,actor_id,client_id,result_json FROM awr_team.planning_command_receipts
         WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3", &[&tenant,&project,&req.request_id],
    ).await?;
    let Some(row) = row else {
        return Ok((None, None));
    };
    let body: Value = row.get(4);
    let canonical = body.get("canonical_request").ok_or_else(|| {
        PgError::WritebackRefused("reserved planning request has unknown original content".into())
    })?;
    if row.get::<_, String>(0) != "planning.activate"
        || row.get::<_, String>(2) != auth.actor_id
        || row.get::<_, String>(3) != auth.client_id
        || canonical["candidate_id"] != publication.candidate_id
        || canonical["candidate_digest"] != publication.candidate_digest
        || canonical["activate"] != true
        || canonical["impact_proven"] != req.impact_proven
        || canonical["stopped_work_ids"] != json!(req.stopped_work_ids)
        || canonical["publish_receipt_id"]
            .as_str()
            .is_some_and(|id| id != req.publish_receipt_id)
        || crate::source::planning_ops::intent_hash("planning.activate", canonical)?
            != row.get::<_, String>(1)
    {
        return Err(PgError::IdempotencyConflict);
    };
    Ok((Some(canonical.clone()), Some(row.get(1))))
}

impl Intent {
    pub fn hash(&self) -> PgResult<String> {
        Ok(super::super::sha256_hex(
            &serde_json::to_vec(self).map_err(|_| PgError::SourceDivergence)?,
        ))
    }

    pub fn require_original(
        &self,
        req: &WritebackActivateRequest,
        auth: &ReaderAuthority,
    ) -> PgResult<()> {
        if self.codec != "awr-planning-writeback-intent-v1"
            || serde_json::to_value(&self.request).map_err(|_| PgError::SourceDivergence)?
                != serde_json::to_value(req).map_err(|_| PgError::SourceDivergence)?
            || self.actor_id != auth.actor_id
            || self.client_id != auth.client_id
        {
            return Err(PgError::WritebackRefused(
                "request_id is bound to a different original intent or identity".into(),
            ));
        }
        Ok(())
    }

    pub fn require_baseline(
        &self,
        auth: &ReaderAuthority,
        epoch: &str,
        registered: &Option<SoleSourceBinding>,
    ) -> PgResult<()> {
        if self.base_snapshot_id != auth.snapshot
            || self.base_authority_epoch != epoch
            || self.coordinator_epoch != auth.epoch
            || &self.registered_source != registered
        {
            return Err(PgError::SourceDivergence);
        }
        Ok(())
    }
}

pub(super) async fn baseline(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    req: &WritebackActivateRequest,
) -> PgResult<(String, Option<SoleSourceBinding>)> {
    let row = tx.query_one(
        "SELECT p.authority_epoch, s.source_ref_json FROM awr_team.projects p
         JOIN awr_team.source_snapshots s ON s.tenant_id=p.tenant_id AND s.project_id=p.id AND s.id=p.active_snapshot_id
         WHERE p.tenant_id=$1 AND p.id=$2", &[&tenant,&project],
    ).await?;
    let value: Value = row.get(1);
    let binding: Option<SoleSourceBinding> = value
        .get("sole_source")
        .filter(|v| !v.is_null())
        .map(|v| serde_json::from_value(v.clone()).map_err(|_| PgError::SourceDivergence))
        .transpose()?;
    if let Some(binding) = &binding {
        if binding.kind != crate::source::SoleSourceKind::ServerDirectory
            || PathBuf::from(&binding.locator) != req.source_root
            || binding
                .ledger_relative_path
                .as_deref()
                .unwrap_or("ledger.yaml")
                != req.ledger_relative_path
        {
            return Err(PgError::WritebackRefused(
                "request does not match the registered sole source".into(),
            ));
        }
    }
    // A missing registration is retained only for the existing operator-local API;
    // MCP/HTTP always resolve a real registration before entering writeback.
    Ok((row.get::<_, i64>(0).to_string(), binding))
}

pub(super) async fn publication(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    receipt: &str,
) -> PgResult<(Publication, Vec<DraftChange>)> {
    let row = tx.query_opt(
        "SELECT p.candidate_id,p.candidate_digest,p.draft_revision,p.approval_id,p.publisher_actor_id,
                a.approver_actor_id,a.candidate_digest,a.candidate_id,a.draft_revision
         FROM awr_team.planning_publish_receipts p JOIN awr_team.planning_approvals a
           ON a.tenant_id=p.tenant_id AND a.project_id=p.project_id AND a.id=p.approval_id
         WHERE p.tenant_id=$1 AND p.project_id=$2 AND p.id=$3 AND p.source_writeback_pending FOR UPDATE OF p",
        &[&tenant,&project,&receipt],
    ).await?.ok_or_else(|| PgError::WritebackRefused("publication is not pending or its exact approval is unavailable".into()))?;
    let candidate_id: String = row.get(0);
    let candidate =
        crate::source::planning::load_candidate(tx, tenant, project, &candidate_id).await?;
    let digest: String = row.get(1);
    let revision: i32 = row.get(2);
    if candidate.state != awr_team::CandidateState::Published
        || candidate
            .candidate_digest()
            .map_err(|_| PgError::StaleApproval)?
            != digest
        || row.get::<_, String>(6) != digest
        || row.get::<_, String>(7) != candidate_id
        || row.get::<_, i32>(8) != revision
        || candidate.draft_revision as i32 != revision
    {
        return Err(PgError::StaleApproval);
    }
    let active = tx.query_one(
        "SELECT s.manifest_digest,p.authority_epoch::text FROM awr_team.projects p
         JOIN awr_team.source_snapshots s ON s.tenant_id=p.tenant_id AND s.project_id=p.id AND s.id=p.active_snapshot_id
         WHERE p.tenant_id=$1 AND p.id=$2", &[&tenant,&project],
    ).await?;
    crate::source::planning::ensure_baseline_current(
        &candidate,
        &active.get::<_, String>(0),
        &active.get::<_, String>(1),
    )?;
    crate::source::planning::authorize_candidate_readable_scope(
        tx, auth, tenant, project, &candidate,
    )
    .await?;
    let approver: String = row.get(5);
    crate::tx::validate_reviewer(tx, tenant, project, &approver).await?;
    Ok((
        Publication {
            candidate_id,
            candidate_digest: digest,
            draft_revision: revision,
            approval_id: row.get(3),
            publisher_actor_id: row.get(4),
            approver_actor_id: approver,
        },
        candidate.changes,
    ))
}
