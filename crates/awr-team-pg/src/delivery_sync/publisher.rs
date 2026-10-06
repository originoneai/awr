//! Recoverable metadata publication, independent of contract activation.
use super::*;
use crate::source::{SoleSourceBinding, SoleSourceKind, SourceFile, SourceStore};
use crate::workstream_auth::ReaderAuthority;
use awr_source::{
    DeliveryCompletionReference, DeliverySourceNote, LockedSourceFile, PublishPrepOptions,
    SoleSourceLocation, SourceFileIdentity, fingerprint, prepare_delivery_source_note,
    prepare_publish_from_ledger_bytes,
};
use awr_team::{Action, WorkstreamBundle};
use serde_json::json;
use std::{collections::BTreeSet, path::Path};
use tokio_postgres::{Row, Transaction};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrepareDeliverySourcePublication {
    pub request_id: String,
    pub read_set: DeliveryReadSet,
    pub candidate_digest: String,
    pub expected_selection_version: String,
    pub expected_metadata_revision: String,
    pub expected_source_fingerprint: String,
    pub completion_receipt_id: Option<String>,
    pub lease_seconds: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryPublicationStep {
    pub request_id: String,
    pub read_set: DeliveryReadSet,
    pub publication_id: String,
    pub fence: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenewDeliveryPublicationLease {
    pub step: DeliveryPublicationStep,
    pub lease_seconds: i32,
}

struct BoundSource {
    location: SoleSourceLocation,
    files: Vec<SourceFile>,
    initial_fingerprint: String,
}

impl BoundSource {
    async fn load(
        tx: &Transaction<'_>,
        tenant: &str,
        project: &str,
        snapshot: &str,
    ) -> PgResult<Self> {
        let row = tx
            .query_opt(
                "SELECT source_ref_json FROM awr_team.source_snapshots
            WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant, &project, &snapshot],
            )
            .await?
            .ok_or(PgError::SourceDivergence)?;
        let source: Value = row.get(0);
        let binding: SoleSourceBinding = serde_json::from_value(
            source
                .get("sole_source")
                .cloned()
                .ok_or(PgError::SourceDivergence)?,
        )
        .map_err(|_| PgError::SourceDivergence)?;
        if binding.kind != SoleSourceKind::ServerDirectory {
            return Err(PgError::Unsupported(
                "delivery metadata writer supports only the bound server-directory YAML source"
                    .into(),
            ));
        }
        let files: Vec<_> = crate::source::files_from_ref(&source)?
            .into_iter()
            .map(|(path, bytes)| SourceFile { path, bytes })
            .collect();
        if SourceStore::validate_publish_package(&files)? != binding {
            return Err(PgError::SourceDivergence);
        }
        let relative = binding
            .ledger_relative_path
            .as_deref()
            .ok_or(PgError::SourceDivergence)?;
        let location = SoleSourceLocation::server_directory(&binding.locator, relative)
            .map_err(|_| PgError::SourceDivergence)?;
        if !relative.ends_with(".yaml") && !relative.ends_with(".yml") {
            return Err(PgError::Unsupported(
                "delivery metadata writer requires a YAML ledger".into(),
            ));
        }
        let initial = files
            .iter()
            .find(|f| f.path == relative)
            .ok_or(PgError::SourceDivergence)?;
        Ok(Self {
            location,
            initial_fingerprint: fingerprint(&initial.bytes),
            files,
        })
    }

    fn relative(&self) -> &str {
        self.location
            .ledger_relative_path
            .as_deref()
            .expect("validated source binding")
    }

    fn open(&self) -> awr_core::Result<LockedSourceFile> {
        LockedSourceFile::open(Path::new(&self.location.locator), self.relative())
    }

    /// Reparse actual bytes and read actual referenced specs through confined opens.
    /// The immutable active package remains the contract/graph authority.
    fn reindex(&self, project: &str, bytes: &[u8]) -> PgResult<Value> {
        awr_core::ensure_public_bytes(bytes).map_err(|_| PgError::SourceDivergence)?;
        let baseline: WorkstreamBundle = serde_json::from_slice(
            &self
                .files
                .iter()
                .find(|f| f.path == crate::source::WORKSTREAMS_FILE)
                .ok_or(PgError::SourceDivergence)?
                .bytes,
        )
        .map_err(|_| PgError::SourceDivergence)?;
        let package = prepare_publish_from_ledger_bytes(
            &self.location,
            Path::new(&self.location.locator),
            bytes,
            project,
            &PublishPrepOptions {
                baseline: Some(baseline.clone()),
                completion_policy: None,
            },
        )
        .map_err(|_| PgError::SourceDivergence)?;
        let actual: Vec<_> = package
            .files
            .iter()
            .map(|f| SourceFile {
                path: f.path.clone(),
                bytes: f.bytes.clone(),
            })
            .collect();
        SourceStore::validate_publish_package(&actual).map_err(|_| PgError::SourceDivergence)?;
        if package.bundle_digest != baseline.hash().map_err(|_| PgError::SourceDivergence)?
            || package.referenced_specs.iter().any(|spec| {
                self.files
                    .iter()
                    .find(|f| f.path == spec.path)
                    .is_none_or(|f| f.bytes != spec.content)
            })
        {
            return Err(PgError::SourceDivergence);
        }
        Ok(json!({"source_fingerprint":package.source_version_digest,
            "bundle_digest":package.bundle_digest,"graph_digest":package.graph_digest,
            "ledger_identity_digest":package.ledger_identity_digest,"parser_version":package.parser_version,
            "referenced_specs_digest":hash(&json!(package.referenced_specs))?,
            "referenced_spec_count":package.referenced_specs.len()}))
    }
}

fn lease(seconds: i32) -> PgResult<()> {
    if !(5..=300).contains(&seconds) {
        return Err(invalid());
    }
    Ok(())
}

fn source_fingerprint(value: &str) -> PgResult<()> {
    digest(value.strip_prefix("sha256:").ok_or_else(invalid)?)
}

async fn current_note(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    set: &DeliveryReadSet,
    publication: &str,
    metadata: i64,
    expected_digest: &str,
    expected_selection: i64,
    completion: Option<&str>,
) -> PgResult<DeliverySourceNote> {
    let (candidate, selected, snapshot, ownership, fence_current, selection_version) =
        snapshots::selection(tx, tenant, project, &set.work_id)
            .await?
            .ok_or(PgError::InactiveCandidate)?;
    auth::bind_candidate(tenant, project, set, &candidate.binding)?;
    if selected != expected_digest
        || snapshot != auth.snapshot
        || ownership != version(&set.ownership_version)?
        || !fence_current
        || selection_version != expected_selection
    {
        return Err(PgError::PreconditionsChanged);
    }
    let rows = tx.query("SELECT f.id,i.id FROM awr_team.delivery_fact_heads h
        JOIN awr_team.delivery_facts f ON f.tenant_id=h.tenant_id AND f.project_id=h.project_id AND f.id=h.fact_id
        JOIN awr_team.delivery_inbox i ON i.tenant_id=f.tenant_id AND i.project_id=f.project_id AND i.id=f.inbox_id
        JOIN awr_team.delivery_inspections x ON x.tenant_id=i.tenant_id AND x.project_id=i.project_id AND x.id=i.inspection_id
        JOIN awr_team.delivery_connectors c ON c.tenant_id=h.tenant_id AND c.project_id=h.project_id AND c.id=h.connector_id
        WHERE h.tenant_id=$1 AND h.project_id=$2 AND h.work_id=$3 AND i.state='applied'
          AND x.binding_digest=$4 AND x.source_snapshot_id=$5 AND x.ownership_version=$6
          AND x.selection_version=$7 AND x.coordinator_epoch=$8 AND c.enabled
          AND c.version=x.connector_version AND c.coordinator_epoch=x.coordinator_epoch
        ORDER BY h.connector_id,h.slot LIMIT 33",
        &[&tenant,&project,&set.work_id,&selected,&auth.snapshot,&ownership,&selection_version,&auth.epoch]).await?;
    if rows.len() > 32 {
        return Err(PgError::ResponseTooLarge);
    }
    let facts: Vec<String> = rows.iter().map(|r| r.get(0)).collect();
    let receipts: Vec<String> = rows
        .iter()
        .map(|r| r.get::<_, String>(1))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let completion_reference = if let Some(id) = completion {
        identity(id)?;
        Some(completion_reference(tx, tenant, project, set, &candidate, &selected, id).await?)
    } else {
        None
    };
    if facts.is_empty() && completion_reference.is_none() {
        return Err(PgError::EvidenceInvalid);
    }
    let note = DeliverySourceNote {
        version: 1,
        publication_id: publication.into(),
        work_external_key: candidate.binding.work_id.as_str().to_owned(),
        contract_snapshot_id: auth.snapshot.clone(),
        candidate_id: candidate.binding.candidate_id.as_str().to_owned(),
        candidate_version: candidate.binding.candidate_version.clone(),
        candidate_digest: selected,
        selection_version: selection_version.to_string(),
        metadata_revision: metadata.to_string(),
        observation_receipt_ids: receipts,
        fact_ids: facts,
        completion_reference,
    };
    // The source key may differ from the domain work ID.
    let external: String = tx
        .query_one(
            "SELECT external_key FROM awr_team.work_items
        WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &set.work_id],
        )
        .await?
        .get(0);
    let note = DeliverySourceNote {
        work_external_key: external,
        ..note
    };
    note.validate().map_err(|_| invalid())?;
    Ok(note)
}

async fn completion_reference(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    set: &DeliveryReadSet,
    candidate: &DeliveryCandidate,
    selected: &str,
    id: &str,
) -> PgResult<DeliveryCompletionReference> {
    let row = tx.query_opt("SELECT e.id,e.digest,e.input_digest,e.output_digest,e.execution_result_digest,
        e.payload_json,a.id,a.sha256,a.content,a.state FROM awr_team.work_runtime w
        JOIN awr_team.completion_receipts c ON c.tenant_id=w.tenant_id AND c.project_id=w.project_id AND c.id=w.selected_completion_id
        JOIN awr_team.evidence e ON e.tenant_id=c.tenant_id AND e.project_id=c.project_id AND e.id=c.evidence_id
        JOIN awr_team.artifacts a ON a.tenant_id=e.tenant_id AND a.project_id=e.project_id AND a.id=e.artifact_id
        WHERE w.tenant_id=$1 AND w.project_id=$2 AND w.scope_id='main' AND w.work_id=$3
          AND w.state='completed' AND NOT w.recovery_blocked AND c.id=$4 AND c.scope_id='main'
          AND c.work_id=$3 AND e.work_id=$3 AND c.contract_hash=$5 AND e.contract_hash=$5
          AND c.delivery_candidate_digest=$6 AND c.result_digest=e.digest AND c.evidence_bundle_hash=e.digest
          AND c.execution_id IS NOT DISTINCT FROM e.execution_id",
        &[&tenant,&project,&set.work_id,&id,&set.contract_hash,&selected]).await?.ok_or(PgError::EvidenceInvalid)?;
    let input: Option<String> = row.get(2);
    let output: Option<String> = row.get(3);
    let result: Option<String> = row.get(4);
    let evidence_digest = crate::review::evidence_digest(
        &set.work_id,
        &set.contract_hash,
        input.as_deref(),
        output.as_deref(),
        result.as_deref(),
        &row.get(5),
    )?;
    let artifact_id: String = row.get(6);
    let sha: String = row.get(7);
    let content: Vec<u8> = row
        .get::<_, Option<Vec<u8>>>(8)
        .ok_or(PgError::EvidenceInvalid)?;
    if row.get::<_, String>(1) != evidence_digest
        || row.get::<_, String>(9) != "finalized"
        || fingerprint(&content) != format!("sha256:{sha}")
        || output.as_deref() != Some(sha.as_str())
        || !candidate.manifest.entries.iter().any(|e| {
            e.artifact_id == artifact_id
                && e.sha256 == sha
                && e.byte_length == content.len().to_string()
        })
    {
        return Err(PgError::EvidenceInvalid);
    }
    Ok(DeliveryCompletionReference {
        receipt_id: id.into(),
        evidence_id: row.get(0),
        result_digest: evidence_digest,
        artifact_id,
        artifact_sha256: sha,
    })
}

struct Journal {
    id: String,
    set: DeliveryReadSet,
    actor: String,
    client: String,
    authority: String,
    note: DeliverySourceNote,
    identity: SourceFileIdentity,
    before: Vec<u8>,
    after: Vec<u8>,
    before_fingerprint: String,
    after_fingerprint: String,
    projection: Value,
    phase: String,
    fence: i64,
    live: bool,
    confirmation: Option<Value>,
    written: bool,
}

impl Journal {
    async fn load(tx: &Transaction<'_>, tenant: &str, project: &str, id: &str) -> PgResult<Self> {
        let row = tx.query_opt("SELECT j.*,expires_at>clock_timestamp() AS lease_live
            FROM awr_team.delivery_source_publications j WHERE tenant_id=$1 AND project_id=$2 AND id=$3 FOR UPDATE",
            &[&tenant,&project,&id]).await?.ok_or(PgError::InactiveCandidate)?;
        Self::from_row(&row)
    }

    fn from_row(row: &Row) -> PgResult<Self> {
        let j = Self {
            id: row.get("id"),
            set: serde_json::from_value(row.get("read_set_json"))
                .map_err(|_| PgError::SourceDivergence)?,
            actor: row.get("actor_id"),
            client: row.get("client_id"),
            authority: row.get("authority_binding"),
            note: serde_json::from_value(row.get("note_json"))
                .map_err(|_| PgError::SourceDivergence)?,
            identity: serde_json::from_value(row.get("filesystem_identity_json"))
                .map_err(|_| PgError::SourceDivergence)?,
            before: row.get("before_bytes"),
            after: row.get("after_bytes"),
            before_fingerprint: row.get("before_fingerprint"),
            after_fingerprint: row.get("after_fingerprint"),
            projection: row.get("projection_json"),
            phase: row.get("phase"),
            fence: row.get("fence"),
            live: row.get("lease_live"),
            confirmation: row.get("confirmation_json"),
            written: row.get("source_written_observed"),
        };
        j.note.validate().map_err(|_| PgError::SourceDivergence)?;
        if fingerprint(&j.before) != j.before_fingerprint
            || fingerprint(&j.after) != j.after_fingerprint
            || j.note.publication_id != j.id
            || j.note.contract_snapshot_id != j.set.source_snapshot_id
            || j.note.candidate_digest != row.get::<_, String>("candidate_digest")
            || version(&j.note.selection_version)? != row.get::<_, i64>("selection_version")
            || version(&j.note.metadata_revision)? != row.get::<_, i64>("metadata_revision")
            || j.set.work_id != row.get::<_, String>("work_id")
            || j.set.source_snapshot_id != row.get::<_, String>("source_snapshot_id")
        {
            return Err(PgError::SourceDivergence);
        }
        Ok(j)
    }

    async fn require(
        &self,
        tx: &Transaction<'_>,
        tenant: &str,
        project: &str,
        auth: &ReaderAuthority,
        step: &DeliveryPublicationStep,
        require_live: bool,
        require_note: bool,
    ) -> PgResult<()> {
        if self.actor != auth.actor_id || self.client != auth.client_id {
            return Err(PgError::Forbidden);
        }
        if json!(self.set) != json!(step.read_set)
            || self.authority != auth::authority_binding(auth, &step.read_set)?
        {
            return Err(PgError::PreconditionsChanged);
        }
        if self.fence != version(&step.fence)? {
            return Err(PgError::StaleFence);
        }
        if require_live && !self.live {
            return Err(PgError::LeaseExpired);
        }
        if self.phase == "confirmed" || self.phase == "conflict" && require_note {
            return Err(PgError::PreconditionsChanged);
        }
        let row = tx
            .query_one(
                "SELECT pending_publication_id,last_fence FROM awr_team.delivery_source_cursors
            WHERE tenant_id=$1 AND project_id=$2 AND source_snapshot_id=$3 FOR UPDATE",
                &[&tenant, &project, &self.set.source_snapshot_id],
            )
            .await?;
        if row.get::<_, Option<String>>(0).as_deref() != Some(self.id.as_str())
            || row.get::<_, i64>(1) != self.fence
        {
            return Err(PgError::StaleFence);
        }
        if require_note {
            self.require_note(tx, tenant, project, auth).await?;
        }
        Ok(())
    }

    async fn require_note(
        &self,
        tx: &Transaction<'_>,
        tenant: &str,
        project: &str,
        auth: &ReaderAuthority,
    ) -> PgResult<()> {
        let actual = current_note(
            tx,
            tenant,
            project,
            auth,
            &self.set,
            &self.id,
            version(&self.note.metadata_revision)?,
            &self.note.candidate_digest,
            version(&self.note.selection_version)?,
            self.note
                .completion_reference
                .as_ref()
                .map(|c| c.receipt_id.as_str()),
        )
        .await?;
        if actual != self.note {
            return Err(PgError::PreconditionsChanged);
        }
        Ok(())
    }

    fn summary(&self) -> Value {
        json!({"publication_id":self.id,"phase":self.phase,"work_id":self.set.work_id,
            "contract_snapshot_id":self.set.source_snapshot_id,"metadata_revision":self.note.metadata_revision,
            "candidate_digest":self.note.candidate_digest,"selection_version":self.note.selection_version,
            "before_fingerprint":self.before_fingerprint,"after_fingerprint":self.after_fingerprint,
            "fence":self.fence.to_string(),"lease_live":self.live,
            "confirmation":self.confirmation,"source_written_observed":self.written,
            "source_synchronized_at_commit":self.phase=="confirmed"})
    }
}

async fn mark(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    id: &str,
    phase: &str,
    code: Option<&str>,
    observed: Option<&str>,
    confirmation: Option<&Value>,
) -> PgResult<()> {
    tx.execute(
        "UPDATE awr_team.delivery_source_publications SET phase=$4,failure_code=$5,
        observed_fingerprint=$6,confirmation_json=$7,updated_at=clock_timestamp(),
        source_written_observed=source_written_observed OR $4 IN ('source_written','confirmed')
            OR COALESCE($6=after_fingerprint,false)
        WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
        &[
            &tenant,
            &project,
            &id,
            &phase,
            &code,
            &observed,
            &confirmation,
        ],
    )
    .await?;
    Ok(())
}

impl DeliverySyncStore {
    /// Persist the exact before/after intent before any source-byte effect.
    pub async fn prepare_source_publication(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        request: PrepareDeliverySourcePublication,
    ) -> PgResult<Value> {
        bounded(&request)?;
        digest(&request.candidate_digest)?;
        source_fingerprint(&request.expected_source_fingerprint)?;
        lease(request.lease_seconds)?;
        let metadata = version(&request.expected_metadata_revision)?;
        let selection = version(&request.expected_selection_version)?;
        if selection <= 0 || metadata < 0 {
            return Err(invalid());
        }
        let set = &request.read_set;
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let auth = auth::admit(
            &tx,
            tenant,
            project,
            bearer,
            set,
            Action::AccessManageProject,
            None,
        )
        .await?;
        let op = "delivery.source.prepare";
        let (hash, replayed) = auth::replay(
            &tx,
            tenant,
            project,
            &auth,
            op,
            &request.request_id,
            &request,
        )
        .await?;
        if let Some(result) = replayed {
            tx.commit().await?;
            return Ok(result);
        }
        let bound = BoundSource::load(&tx, tenant, project, &auth.snapshot).await?;
        tx.execute("INSERT INTO awr_team.delivery_source_cursors(tenant_id,project_id,source_snapshot_id,confirmed_fingerprint)
            VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING", &[&tenant,&project,&auth.snapshot,&bound.initial_fingerprint]).await?;
        let cursor=tx.query_one("SELECT metadata_revision,confirmed_fingerprint,last_fence,pending_publication_id
            FROM awr_team.delivery_source_cursors WHERE tenant_id=$1 AND project_id=$2 AND source_snapshot_id=$3 FOR UPDATE",
            &[&tenant,&project,&auth.snapshot]).await?;
        if cursor.get::<_, Option<String>>(3).is_some() {
            return Err(PgError::ResourceConflict);
        }
        if cursor.get::<_, i64>(0) != metadata
            || cursor.get::<_, String>(1) != request.expected_source_fingerprint
        {
            return Err(PgError::PreconditionsChanged);
        }
        let guard = bound.open().map_err(|_| PgError::SourceDivergence)?;
        let before = guard.read().map_err(|_| PgError::SourceDivergence)?;
        if fingerprint(&before) != request.expected_source_fingerprint {
            return Err(PgError::SourceDivergence);
        }
        bound.reindex(project, &before)?;
        let id = crate::tx::new_id();
        let next = metadata
            .checked_add(1)
            .ok_or(PgError::PreconditionsChanged)?;
        let fence = cursor
            .get::<_, i64>(2)
            .checked_add(1)
            .ok_or(PgError::PreconditionsChanged)?;
        let note = current_note(
            &tx,
            tenant,
            project,
            &auth,
            set,
            &id,
            next,
            &request.candidate_digest,
            selection,
            request.completion_receipt_id.as_deref(),
        )
        .await?;
        let patch =
            prepare_delivery_source_note(&before, &note).map_err(|_| PgError::SourceDivergence)?;
        let projection = bound.reindex(project, &patch.after_bytes)?;
        if guard.read().map_err(|_| PgError::SourceDivergence)? != before {
            return Err(PgError::SourceDivergence);
        }
        tx.execute("INSERT INTO awr_team.delivery_source_publications(tenant_id,project_id,id,source_snapshot_id,work_id,
            actor_id,client_id,authority_binding,read_set_json,candidate_digest,selection_version,metadata_revision,note_json,
            filesystem_identity_json,before_fingerprint,after_fingerprint,before_bytes,after_bytes,projection_json,fence,expires_at)
            VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,clock_timestamp()+make_interval(secs=>$21::integer))",
            &[&tenant,&project,&id,&auth.snapshot,&set.work_id,&auth.actor_id,&auth.client_id,&auth::authority_binding(&auth,set)?,
            &json!(set),&request.candidate_digest,&selection,&next,&json!(note),&json!(guard.identity()),
            &patch.before_fingerprint,&patch.after_fingerprint,&patch.before_bytes,&patch.after_bytes,&projection,&fence,&request.lease_seconds]).await?;
        tx.execute(
            "UPDATE awr_team.delivery_source_cursors SET pending_publication_id=$4,last_fence=$5
            WHERE tenant_id=$1 AND project_id=$2 AND source_snapshot_id=$3",
            &[&tenant, &project, &auth.snapshot, &id, &fence],
        )
        .await?;
        let data = Journal::load(&tx, tenant, project, &id).await?.summary();
        let result = auth::finish(
            &tx,
            tenant,
            project,
            &auth,
            set,
            op,
            &request.request_id,
            &hash,
            data,
        )
        .await?;
        tx.commit().await?;
        Ok(result)
    }

    /// Same intent/owner, a new monotonic fence. No expired worker is revived.
    pub async fn renew_source_publication(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        request: RenewDeliveryPublicationLease,
    ) -> PgResult<Value> {
        bounded(&request)?;
        lease(request.lease_seconds)?;
        let step = &request.step;
        identity(&step.publication_id)?;
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let auth = auth::admit(
            &tx,
            tenant,
            project,
            bearer,
            &step.read_set,
            Action::AccessManageProject,
            None,
        )
        .await?;
        let op = "delivery.source.renew";
        let (hash, replayed) =
            auth::replay(&tx, tenant, project, &auth, op, &step.request_id, &request).await?;
        if let Some(result) = replayed {
            tx.commit().await?;
            return Ok(result);
        }
        let j = Journal::load(&tx, tenant, project, &step.publication_id).await?;
        j.require(&tx, tenant, project, &auth, step, false, true)
            .await?;
        let fence = j.fence.checked_add(1).ok_or(PgError::StaleFence)?;
        tx.execute("UPDATE awr_team.delivery_source_publications SET fence=$4,
            expires_at=clock_timestamp()+make_interval(secs=>$5::integer),updated_at=clock_timestamp()
            WHERE tenant_id=$1 AND project_id=$2 AND id=$3", &[&tenant,&project,&j.id,&fence,&request.lease_seconds]).await?;
        tx.execute(
            "UPDATE awr_team.delivery_source_cursors SET last_fence=$4
            WHERE tenant_id=$1 AND project_id=$2 AND source_snapshot_id=$3",
            &[&tenant, &project, &auth.snapshot, &fence],
        )
        .await?;
        let data = Journal::load(&tx, tenant, project, &j.id).await?.summary();
        let result = auth::finish(
            &tx,
            tenant,
            project,
            &auth,
            &step.read_set,
            op,
            &step.request_id,
            &hash,
            data,
        )
        .await?;
        tx.commit().await?;
        Ok(result)
    }

    /// Observe first: before may be written, after is never written again.
    pub async fn write_source_publication(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        request: DeliveryPublicationStep,
    ) -> PgResult<Value> {
        self.publication_step(tenant, project, bearer, request, false)
            .await
    }

    /// Reindex the actual resulting bytes; never creates a source approval,
    /// activates a contract, finalizes work or treats an external merge as done.
    pub async fn confirm_source_publication(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        request: DeliveryPublicationStep,
    ) -> PgResult<Value> {
        self.publication_step(tenant, project, bearer, request, true)
            .await
    }

    async fn publication_step(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        request: DeliveryPublicationStep,
        confirm: bool,
    ) -> PgResult<Value> {
        bounded(&request)?;
        identity(&request.publication_id)?;
        let set = &request.read_set;
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let auth = auth::admit(
            &tx,
            tenant,
            project,
            bearer,
            set,
            Action::AccessManageProject,
            None,
        )
        .await?;
        let op = if confirm {
            "delivery.source.confirm"
        } else {
            "delivery.source.write"
        };
        let (hash, replayed) = auth::replay(
            &tx,
            tenant,
            project,
            &auth,
            op,
            &request.request_id,
            &request,
        )
        .await?;
        if let Some(result) = replayed {
            tx.commit().await?;
            return Ok(result);
        }
        let j = Journal::load(&tx, tenant, project, &request.publication_id).await?;
        j.require(&tx, tenant, project, &auth, &request, true, true)
            .await?;
        let bound = BoundSource::load(&tx, tenant, project, &auth.snapshot).await?;
        // The handle and project authority locks remain held through the effect
        // and its confirmation. The source lock is nonblocking to avoid inversion.
        let source_guard = bound.open();
        let (phase, code, observed, confirmation) = match &source_guard {
            Err(_) => ("failed", Some("source_unavailable"), None, None),
            Ok(guard) if guard.verify_identity(&j.identity).is_err() => {
                ("conflict", Some("source_identity_changed"), None, None)
            }
            Ok(guard) => {
                self.apply_observed_source(&tx, tenant, project, &auth, &bound, &guard, &j, confirm)
                    .await?
            }
        };
        mark(
            &tx,
            tenant,
            project,
            &j.id,
            phase,
            code,
            observed.as_deref(),
            confirmation.as_ref(),
        )
        .await?;
        if phase == "confirmed" {
            tx.execute("UPDATE awr_team.delivery_source_cursors SET metadata_revision=$4,confirmed_fingerprint=$5,pending_publication_id=NULL
                WHERE tenant_id=$1 AND project_id=$2 AND source_snapshot_id=$3 AND pending_publication_id=$6 AND last_fence=$7",
                &[&tenant,&project,&auth.snapshot,&version(&j.note.metadata_revision)?,&j.after_fingerprint,&j.id,&j.fence]).await?;
        }
        let mut data = Journal::load(&tx, tenant, project, &j.id).await?.summary();
        data["failure_code"] = json!(code);
        data["observed_fingerprint"] = json!(observed);
        let result = auth::finish(
            &tx,
            tenant,
            project,
            &auth,
            set,
            op,
            &request.request_id,
            &hash,
            data,
        )
        .await?;
        tx.commit().await?;
        drop(source_guard);
        Ok(result)
    }

    async fn apply_observed_source(
        &self,
        tx: &Transaction<'_>,
        tenant: &str,
        project: &str,
        auth: &ReaderAuthority,
        bound: &BoundSource,
        guard: &LockedSourceFile,
        j: &Journal,
        confirm: bool,
    ) -> PgResult<(
        &'static str,
        Option<&'static str>,
        Option<String>,
        Option<Value>,
    )> {
        let Ok(mut actual) = guard.read() else {
            return Ok(("failed", Some("source_unavailable"), None, None));
        };
        let mut observed = fingerprint(&actual);
        if observed != j.before_fingerprint && observed != j.after_fingerprint {
            return Ok(("conflict", Some("source_drift"), Some(observed), None));
        }
        if observed == j.before_fingerprint {
            if j.written {
                return Ok(("conflict", Some("source_drift"), Some(observed), None));
            }
            if confirm {
                return Ok(("pending", None, Some(observed), None));
            }
            if bound.reindex(project, &actual).is_err() {
                return Ok(("conflict", Some("projection_changed"), Some(observed), None));
            }
            if guard.replace(&j.before_fingerprint, &j.after).is_err() {
                let observed = guard.read().ok().map(|b| fingerprint(&b));
                let phase = if observed.as_deref() == Some(&j.after_fingerprint) {
                    "source_written"
                } else if observed.as_deref() == Some(&j.before_fingerprint) {
                    "failed"
                } else {
                    "conflict"
                };
                return Ok((phase, Some("write_failed"), observed, None));
            }
            // A successful replacement is a known effect even if a subsequent
            // read or projection check fails. Retain it with the phase commit.
            tx.execute(
                "UPDATE awr_team.delivery_source_publications SET source_written_observed=true
                WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant, &project, &j.id],
            )
            .await?;
            let Ok(bytes) = guard.read() else {
                return Ok(("failed", Some("source_unavailable"), None, None));
            };
            actual = bytes;
            observed = fingerprint(&actual);
        }
        if observed != j.after_fingerprint {
            return Ok(("conflict", Some("source_drift"), Some(observed), None));
        }
        let live: bool = tx
            .query_one(
                "SELECT expires_at>clock_timestamp() FROM awr_team.delivery_source_publications
            WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant, &project, &j.id],
            )
            .await?
            .get(0);
        if !live {
            return Ok((
                "source_written",
                Some("lease_expired"),
                Some(observed),
                None,
            ));
        }
        if !confirm {
            return Ok(("source_written", None, Some(observed), None));
        }
        let proof = match bound.reindex(project, &actual) {
            Ok(proof) if proof == j.projection => proof,
            _ => return Ok(("conflict", Some("projection_changed"), Some(observed), None)),
        };
        if guard.read().ok().as_deref() != Some(actual.as_slice())
            || guard.verify_identity(&j.identity).is_err()
        {
            return Ok(("conflict", Some("source_drift"), Some(observed), None));
        }
        // Refresh current candidate, selected observations and domain receipt
        // after reindexing, before committing the synchronized metadata cursor.
        j.require_note(tx, tenant, project, auth).await?;
        let live: bool = tx
            .query_one(
                "SELECT expires_at>clock_timestamp() FROM awr_team.delivery_source_publications
            WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant, &project, &j.id],
            )
            .await?
            .get(0);
        if !live {
            return Ok((
                "source_written",
                Some("lease_expired"),
                Some(observed),
                None,
            ));
        }
        Ok((
            "confirmed",
            None,
            Some(observed),
            Some(json!({"publication_id":j.id,
            "contract_snapshot_id":auth.snapshot,"metadata_revision":j.note.metadata_revision,
            "candidate_digest":j.note.candidate_digest,"selection_version":j.note.selection_version,
            "source_fingerprint":j.after_fingerprint,"projection":proof,
            "completion_reference":j.note.completion_reference,"source_synchronized_at_commit":true,
            "contract_activated":false,"work_finalized":false})),
        ))
    }

    /// Withdraw only after observing the exact unwritten before image. A
    /// changed candidate can be replanned without stealing an unknown effect.
    pub async fn abandon_source_publication(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        request: DeliveryPublicationStep,
    ) -> PgResult<Value> {
        bounded(&request)?;
        identity(&request.publication_id)?;
        let set = &request.read_set;
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let auth = auth::admit(
            &tx,
            tenant,
            project,
            bearer,
            set,
            Action::AccessManageProject,
            None,
        )
        .await?;
        let op = "delivery.source.abandon";
        let (hash, replayed) = auth::replay(
            &tx,
            tenant,
            project,
            &auth,
            op,
            &request.request_id,
            &request,
        )
        .await?;
        if let Some(result) = replayed {
            tx.commit().await?;
            return Ok(result);
        }
        let j = Journal::load(&tx, tenant, project, &request.publication_id).await?;
        j.require(&tx, tenant, project, &auth, &request, false, false)
            .await?;
        let bound = BoundSource::load(&tx, tenant, project, &auth.snapshot).await?;
        let guard = bound.open().map_err(|_| PgError::SourceDivergence)?;
        guard
            .verify_identity(&j.identity)
            .map_err(|_| PgError::SourceDivergence)?;
        let actual = guard.read().map_err(|_| PgError::SourceDivergence)?;
        if j.written || fingerprint(&actual) != j.before_fingerprint {
            return Err(PgError::SourceDivergence);
        }
        mark(
            &tx,
            tenant,
            project,
            &j.id,
            "conflict",
            Some("withdrawn"),
            Some(&j.before_fingerprint),
            None,
        )
        .await?;
        tx.execute("UPDATE awr_team.delivery_source_cursors SET pending_publication_id=NULL
            WHERE tenant_id=$1 AND project_id=$2 AND source_snapshot_id=$3 AND pending_publication_id=$4",
            &[&tenant,&project,&auth.snapshot,&j.id]).await?;
        let mut data = Journal::load(&tx, tenant, project, &j.id).await?.summary();
        data["failure_code"] = json!("withdrawn");
        data["source_bytes_written"] = json!(false);
        let result = auth::finish(
            &tx,
            tenant,
            project,
            &auth,
            set,
            op,
            &request.request_id,
            &hash,
            data,
        )
        .await?;
        tx.commit().await?;
        Ok(result)
    }

    /// Read history and present physical-source currentness without changing
    /// database state, files, claims, approvals or contract activation.
    pub async fn source_publication_status(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        work: &str,
    ) -> PgResult<Value> {
        identity(work)?;
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let mut auth = crate::workstream_auth::authenticate(&tx, tenant, project, bearer).await?;
        crate::delegation_auth::resolve_agent_delegation(
            &tx,
            &mut auth,
            project,
            Some(work),
            None,
            Some(Action::WorkRead),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
        )
        .await?;
        let (binding, ownership) =
            crate::workstream_read::work_binding(&tx, tenant, project, &auth, work).await?;
        crate::workstream_auth::authorize_domain_action(
            &auth,
            Action::WorkRead,
            Some(binding.workstream_id),
            Some(work),
        )?;
        let contract:String=tx.query_one("SELECT contract_hash FROM awr_team.work_contracts
            WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND scope_id='main' AND work_id=$4",
            &[&tenant,&project,&auth.snapshot,&work]).await?.get(0);
        let set = DeliveryReadSet {
            work_id: work.into(),
            workstream_id: binding.workstream_id,
            coordinator_epoch: auth.epoch.clone(),
            source_snapshot_id: auth.snapshot.clone(),
            authority_version: auth
                .catalog
                .get(binding.workstream_id)?
                .authority_version
                .to_string(),
            ownership_version: ownership.to_string(),
            contract_hash: contract,
        };
        let bound = BoundSource::load(&tx, tenant, project, &auth.snapshot).await?;
        let cursor=tx.query_opt("SELECT metadata_revision,confirmed_fingerprint,pending_publication_id
            FROM awr_team.delivery_source_cursors WHERE tenant_id=$1 AND project_id=$2 AND source_snapshot_id=$3",
            &[&tenant,&project,&auth.snapshot]).await?;
        let (metadata, confirmed, pending) = cursor
            .map(|r| {
                (
                    r.get::<_, i64>(0),
                    r.get::<_, String>(1),
                    r.get::<_, Option<String>>(2),
                )
            })
            .unwrap_or((0, bound.initial_fingerprint.clone(), None));
        let physical =
            awr_source::read_under_root(Path::new(&bound.location.locator), bound.relative()).ok();
        let actual_fingerprint = physical.as_ref().map(|b| fingerprint(b));
        let projection_current = physical
            .as_ref()
            .is_some_and(|b| bound.reindex(project, b).is_ok());
        let rows=tx.query("SELECT j.*,expires_at>clock_timestamp() AS lease_live FROM awr_team.delivery_source_publications j
            WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 ORDER BY created_at DESC,id DESC LIMIT 33",
            &[&tenant,&project,&work]).await?;
        let truncated = rows.len() > 32;
        let mut history = Vec::new();
        for row in rows.into_iter().take(32) {
            let j = Journal::from_row(&row)?;
            let current = j.phase == "confirmed"
                && j.set.source_snapshot_id == auth.snapshot
                && projection_current
                && actual_fingerprint.as_deref() == Some(confirmed.as_str())
                && physical.as_ref().is_some_and(|b| {
                    prepare_delivery_source_note(b, &j.note)
                        .is_ok_and(|patch| patch.after_bytes == *b)
                })
                && LockedSourceFile::observe(
                    Path::new(&bound.location.locator),
                    bound.relative(),
                    &j.identity,
                )
                .is_ok_and(|bytes| physical.as_ref() == Some(&bytes))
                && j.require_note(&tx, tenant, project, &auth).await.is_ok();
            let mut item = j.summary();
            item["source_current"] = json!(current);
            item["failure_code"] = json!(row.get::<_, Option<String>>("failure_code"));
            history.push(item);
        }
        let current = history.iter().any(|h| h["source_current"] == true);
        let result = json!({"work_id":work,"read_set":set,"metadata_revision":metadata.to_string(),
            "confirmed_fingerprint":confirmed,"source_fingerprint":actual_fingerprint,
            "source_observation":if physical.is_some(){"observed"}else{"unavailable"},
            "projection_current":projection_current,"cursor_current":actual_fingerprint.as_deref()==Some(confirmed.as_str()),
            "pending_publication_id":pending,"history":history,"history_truncated":truncated,
            "source_synchronized":current,"acceptance_ready":false,"execution_authorized":false});
        if serde_json::to_vec(&result).map_err(|_| invalid())?.len() > 262144 {
            return Err(PgError::ResponseTooLarge);
        }
        tx.commit().await?;
        Ok(result)
    }
}
