//! Durable, explicitly authorized worker leases. No scheduler or repository effect.
use super::*;
use crate::workstream_auth::{ReaderAuthority, authenticate};
use awr_core::WorkstreamAction;
use awr_team::Action;
use serde_json::json;
use tokio_postgres::{IsolationLevel, Row, Transaction};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimDeliverySyncIntent {
    pub request_id: String,
    pub read_set: DeliveryReadSet,
    pub intent_id: String,
    pub worker_id: String,
    pub expected_fence: String,
    pub lease_seconds: i32,
}

/// Opaque server capability, created only by a current authenticated lease.
/// It is never deserialized from an MCP body or persisted with a credential.
#[derive(Clone, Debug)]
pub struct DeliverySyncLease {
    pub(super) tenant: String,
    pub(super) project: String,
    pub(super) id: String,
    pub(super) set: DeliveryReadSet,
    pub(super) worker: String,
    pub(super) fence: i64,
    credential_hash: String,
}

impl DeliverySyncLease {
    pub fn intent_id(&self) -> &str {
        &self.id
    }

    pub fn fence(&self) -> String {
        self.fence.to_string()
    }

    pub(super) fn request(&self, step: &str) -> String {
        format!("pump-{}-{}-{step}", self.id, self.fence)
    }
}

struct Intent {
    id: String,
    notification: String,
    kind: String,
    set: DeliveryReadSet,
    origin: acceptance::Origin,
    completion: Option<String>,
    connector: Option<String>,
    connector_version: Option<i64>,
    generation: Option<i64>,
    candidate: String,
    selection: i64,
    state: String,
    fence: i64,
    worker: Option<String>,
    actor: Option<String>,
    client: Option<String>,
    authority: Option<String>,
    live: bool,
    due: bool,
    publication: Option<String>,
}

impl Intent {
    fn from_row(row: &Row) -> PgResult<Self> {
        Ok(Self {
            id: row.get("id"),
            notification: row.get("notification_id"),
            kind: row.get("kind"),
            origin: acceptance::Origin::parse(row.get("origin"))?,
            completion: row.get("completion_receipt_id"),
            set: serde_json::from_value(row.get("read_set_json"))
                .map_err(|_| PgError::SourceDivergence)?,
            connector: row.get("connector_id"),
            connector_version: row.get("connector_version"),
            generation: row.get("generation"),
            candidate: row.get("candidate_digest"),
            selection: row.get("selection_version"),
            state: row.get("state"),
            fence: row.get("fence"),
            worker: row.get("worker_id"),
            actor: row.get("actor_id"),
            client: row.get("client_id"),
            authority: row.get("authority_binding"),
            live: row.get("lease_live"),
            due: row.get("retry_due"),
            publication: row.get("publication_id"),
        })
    }

    async fn load(tx: &Transaction<'_>, tenant: &str, project: &str, id: &str) -> PgResult<Self> {
        let row = tx
            .query_opt(
                "SELECT i.*,COALESCE(expires_at>clock_timestamp(),false) AS lease_live,
            retry_at<=clock_timestamp() AS retry_due FROM awr_team.delivery_sync_intents i
            WHERE tenant_id=$1 AND project_id=$2 AND id=$3 FOR UPDATE",
                &[&tenant, &project, &id],
            )
            .await?
            .ok_or(PgError::Forbidden)?;
        Self::from_row(&row)
    }

    fn summary(&self) -> Value {
        json!({"intent_id":self.id,"notification_id":self.notification,"kind":self.kind,
            "origin":self.origin,"completion_receipt_id":self.completion,
            "work_id":self.set.work_id,"read_set":self.set,"connector_id":self.connector,
            "connector_version":self.connector_version.map(|v|v.to_string()),"generation":self.generation.map(|v|v.to_string()),
            "candidate_digest":self.candidate,"selection_version":self.selection.to_string(),
            "state":self.state,"fence":self.fence.to_string(),"worker_id":self.worker,
            "lease_live":self.live,"retry_due":self.due,"publication_id":self.publication,
            "execution_authorized":false,"acceptance_ready":false})
    }

    async fn current(&self, tx: &Transaction<'_>, tenant: &str, project: &str) -> PgResult<bool> {
        if self.origin == acceptance::Origin::DomainAcceptance {
            let pending: bool = tx.query_one("SELECT EXISTS(SELECT 1 FROM awr_team.delivery_notifications
                WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND origin='domain_acceptance'
                  AND completion_receipt_id=$4 AND work_id=$5 AND state IN ('pending','delivered'))",
                &[&tenant,&project,&self.notification,&self.completion,&self.set.work_id]).await?.get(0);
            return Ok(pending
                && acceptance::current(
                    tx,
                    tenant,
                    project,
                    &self.set,
                    self.completion
                        .as_deref()
                        .ok_or(PgError::SourceDivergence)?,
                    &self.candidate,
                    self.selection,
                )
                .await?);
        }
        let current: bool = tx.query_one("SELECT EXISTS(SELECT 1 FROM awr_team.delivery_connectors c
            JOIN awr_team.delivery_selections s ON (s.tenant_id,s.project_id,s.work_id)=(c.tenant_id,c.project_id,c.work_id)
            JOIN awr_team.work_runtime w ON (w.tenant_id,w.project_id,w.scope_id,w.work_id)=(s.tenant_id,s.project_id,s.scope_id,s.work_id)
            JOIN awr_team.delivery_notifications n ON (n.tenant_id,n.project_id,n.id)=(c.tenant_id,c.project_id,$3)
            WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.id=$4 AND c.enabled
            AND c.version=$5 AND c.inspection_generation=$6 AND c.coordinator_epoch=$7
            AND s.binding_digest=$8 AND s.selection_version=$9 AND s.source_snapshot_id=$10
            AND s.ownership_version=$11 AND w.last_fence=s.fence AND n.state IN ('pending','delivered'))",
            &[&tenant,&project,&self.notification,&self.connector,&self.connector_version,&self.generation,
              &self.set.coordinator_epoch,&self.candidate,&self.selection,&self.set.source_snapshot_id,
              &version(&self.set.ownership_version)?]).await?.get(0);
        Ok(current)
    }

    /// An observed current read set is offered only before any source journal.
    /// The claim transaction repeats these proofs before persisting a rebind.
    async fn effective_set(
        &self,
        tx: &Transaction<'_>,
        tenant: &str,
        project: &str,
        auth: &ReaderAuthority,
    ) -> PgResult<DeliveryReadSet> {
        let mut set = self.set.clone();
        if self.origin == acceptance::Origin::DomainAcceptance
            && self.publication.is_none()
            && self.set.source_snapshot_id != auth.snapshot
            && self.set.coordinator_epoch == auth.epoch
        {
            let (_, ownership) =
                crate::workstream_read::work_binding(tx, tenant, project, auth, &self.set.work_id)
                    .await?;
            if version(&self.set.ownership_version)? == ownership
                && version(&self.set.authority_version)? as u64
                    == auth.catalog.get(self.set.workstream_id)?.authority_version
            {
                match completion::require_contracts(
                    tx,
                    tenant,
                    project,
                    &self.set.work_id,
                    &self.set.source_snapshot_id,
                    &auth.snapshot,
                    &self.set.contract_hash,
                )
                .await
                {
                    Ok(()) => set.source_snapshot_id = auth.snapshot.clone(),
                    Err(PgError::PreconditionsChanged) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(set)
    }
}

fn lease_seconds(value: i32) -> PgResult<()> {
    if !(5..=300).contains(&value) {
        return Err(invalid());
    }
    Ok(())
}

pub(super) async fn enqueue(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    notification: &str,
    request: &IngestDeliveryFacts,
    connector_version: i64,
    generation: i64,
    candidate: &str,
    selection: i64,
) -> PgResult<()> {
    for kind in ["refresh", "source"] {
        tx.execute("INSERT INTO awr_team.delivery_sync_intents(tenant_id,project_id,id,notification_id,kind,
            work_id,connector_id,connector_version,generation,candidate_digest,selection_version,read_set_json)
            VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
            &[&tenant,&project,&crate::tx::new_id(),&notification,&kind,&request.read_set.work_id,
              &request.connector_id,&connector_version,&generation,&candidate,&selection,&json!(request.read_set)])
            .await?;
    }
    Ok(())
}

/// Called within the publisher/acknowledgement transaction, never as a preflight.
pub(super) async fn require(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    lease: &DeliverySyncLease,
    publication: Option<&str>,
) -> PgResult<()> {
    if lease.tenant != tenant || lease.project != project {
        return Err(PgError::Forbidden);
    }
    // Identity rows remain locked by admission, but a credential's time limit
    // can elapse during filesystem processing without a concurrent revocation.
    let credential_live: bool = tx
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM awr_team.credentials
        WHERE tenant_id=$1 AND actor_id=$2 AND client_id=$3 AND secret_hash=$4
          AND (project_id IS NULL OR project_id=$5) AND revoked_at IS NULL
          AND (expires_at IS NULL OR expires_at>clock_timestamp()))",
            &[
                &tenant,
                &auth.actor_id,
                &auth.client_id,
                &lease.credential_hash,
                &project,
            ],
        )
        .await?
        .get(0);
    if !credential_live {
        return Err(PgError::Forbidden);
    }
    let intent = Intent::load(tx, tenant, project, &lease.id).await?;
    if intent.actor.as_deref() != Some(&auth.actor_id)
        || intent.client.as_deref() != Some(&auth.client_id)
    {
        return Err(PgError::Forbidden);
    }
    if intent.fence != lease.fence || intent.state != "leased" {
        return Err(PgError::StaleFence);
    }
    if intent.worker.as_deref() != Some(&lease.worker) {
        return Err(PgError::Forbidden);
    }
    if !intent.live {
        return Err(PgError::LeaseExpired);
    }
    if json!(intent.set) != json!(lease.set)
        || intent.authority.as_deref() != Some(&auth::authority_binding(auth, &lease.set)?)
        || !intent.current(tx, tenant, project).await?
    {
        return Err(PgError::PreconditionsChanged);
    }
    if let Some(id) = publication {
        if intent.kind != "source" || intent.publication.as_deref() != Some(id) {
            return Err(PgError::Forbidden);
        }
    }
    Ok(())
}

/// Direct APIs cannot omit the worker capability for a pump-owned publication.
pub(super) async fn publication_guard(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    publication: &str,
    guard: Option<&DeliverySyncLease>,
) -> PgResult<()> {
    let owned = tx
        .query_opt(
            "SELECT id FROM awr_team.delivery_sync_intents
        WHERE tenant_id=$1 AND project_id=$2 AND publication_id=$3",
            &[&tenant, &project, &publication],
        )
        .await?;
    match (owned, guard) {
        (Some(row), Some(lease)) if row.get::<_, String>(0) == lease.id => {
            require(tx, tenant, project, auth, lease, Some(publication)).await
        }
        (None, None) => Ok(()),
        _ => Err(PgError::Forbidden),
    }
}

pub(super) async fn bind_publication(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    lease: &DeliverySyncLease,
    publication: &str,
) -> PgResult<()> {
    require(tx, tenant, project, auth, lease, None).await?;
    let changed=tx.execute("UPDATE awr_team.delivery_sync_intents SET publication_id=$4,updated_at=clock_timestamp()
        WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND kind='source' AND publication_id IS NULL",
        &[&tenant,&project,&lease.id,&publication]).await?;
    if changed != 1 {
        return Err(PgError::ResourceConflict);
    }
    Ok(())
}

pub(super) async fn finish_source(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    lease: &DeliverySyncLease,
    publication: &str,
    confirmation: &Value,
) -> PgResult<()> {
    require(tx, tenant, project, auth, lease, Some(publication)).await?;
    tx.execute(
        "UPDATE awr_team.delivery_sync_intents SET state='succeeded',result_json=$4,
        updated_at=clock_timestamp() WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
        &[&tenant, &project, &lease.id, &confirmation],
    )
    .await?;
    Ok(())
}

impl DeliverySyncStore {
    /// A bounded scoped queue read; no lock repair, lease allocation or effect.
    pub async fn sync_intents(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        workstream: Id,
        limit: u16,
        after: Option<&str>,
    ) -> PgResult<Value> {
        if !(1..=64).contains(&limit) {
            return Err(invalid());
        }
        if let Some(id) = after {
            identity(id)?;
        }
        // A write to the project between this read's snapshot and its locks makes PostgreSQL
        // roll the read back (serialization failure). It changes nothing, so it runs again.
        crate::retry_rolled_back(|| {
            self.sync_intents_once(tenant, project, bearer, workstream, limit, after)
        })
        .await
    }

    async fn sync_intents_once(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        workstream: Id,
        limit: u16,
        after: Option<&str>,
    ) -> PgResult<Value> {
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let mut auth = authenticate(&tx, tenant, project, bearer).await?;
        if !matches!(auth.actor_kind.as_str(), "human" | "system") {
            return Err(PgError::Forbidden);
        }
        crate::delegation_auth::authorize_project_action(
            &tx,
            &mut auth,
            project,
            Action::AccessManageProject,
        )
        .await?;
        auth.access
            .authorize(&auth.catalog, workstream, WorkstreamAction::Manage)?;
        auth.access
            .authorize(&auth.catalog, workstream, WorkstreamAction::Read)?;
        let stream = workstream.to_string();
        if let Some(id) = after {
            let visible=tx.query_opt("SELECT i.id FROM awr_team.delivery_sync_intents i
                JOIN awr_team.workstream_snapshot_ownership o ON (o.tenant_id,o.project_id,o.work_id)=(i.tenant_id,i.project_id,i.work_id)
                WHERE i.tenant_id=$1 AND i.project_id=$2 AND i.id=$3 AND o.snapshot_id=$4 AND o.workstream_id=$5",
                &[&tenant,&project,&id,&auth.snapshot,&stream]).await?;
            if visible.is_none() {
                return Err(PgError::Forbidden);
            }
        }
        let rows=tx.query("SELECT i.*,COALESCE(expires_at>clock_timestamp(),false) AS lease_live,
            retry_at<=clock_timestamp() AS retry_due FROM awr_team.delivery_sync_intents i
            JOIN awr_team.workstream_snapshot_ownership o ON (o.tenant_id,o.project_id,o.work_id)=(i.tenant_id,i.project_id,i.work_id)
            WHERE i.tenant_id=$1 AND i.project_id=$2 AND o.snapshot_id=$3 AND o.workstream_id=$4
            AND ($5::text IS NULL OR (i.created_at,i.id)>(SELECT created_at,id FROM awr_team.delivery_sync_intents
                WHERE tenant_id=$1 AND project_id=$2 AND id=$5)) ORDER BY i.created_at,i.id LIMIT $6",
            &[&tenant,&project,&auth.snapshot,&stream,&after,&(i64::from(limit)+1)]).await?;
        let more = rows.len() > usize::from(limit);
        let mut items = Vec::new();
        let mut next = None;
        for row in rows.into_iter().take(usize::from(limit)) {
            let mut intent = Intent::from_row(&row)?;
            let (_, ownership) = crate::workstream_read::work_binding(
                &tx,
                tenant,
                project,
                &auth,
                &intent.set.work_id,
            )
            .await?;
            intent.set = intent.effective_set(&tx, tenant, project, &auth).await?;
            let current = intent.set.source_snapshot_id == auth.snapshot
                && intent.set.coordinator_epoch == auth.epoch
                && version(&intent.set.ownership_version)? == ownership
                && version(&intent.set.authority_version)? as u64
                    == auth.catalog.get(workstream)?.authority_version
                && intent.current(&tx, tenant, project).await?;
            let mut value = intent.summary();
            value["binding_current"] = json!(current);
            value["owned_by_client"] = json!(
                intent.actor.as_deref() == Some(&auth.actor_id)
                    && intent.client.as_deref() == Some(&auth.client_id)
            );
            value["failure_code"] = json!(row.get::<_, Option<String>>("failure_code"));
            next = Some(intent.id);
            items.push(value);
        }
        let result = json!({"intents":items,"has_more":more,"next_after":if more{next}else{None},
            "execution_authorized":false,"background_scheduling":false});
        if serde_json::to_vec(&result).map_err(|_| invalid())?.len() > 262144 {
            return Err(PgError::ResponseTooLarge);
        }
        tx.commit().await?;
        Ok(result)
    }

    /// Stable retries can recover only the same still-current worker capability.
    pub async fn claim_sync_intent(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        request: ClaimDeliverySyncIntent,
    ) -> PgResult<Option<DeliverySyncLease>> {
        bounded(&request)?;
        identity(&request.intent_id)?;
        identity(&request.worker_id)?;
        lease_seconds(request.lease_seconds)?;
        let expected = version(&request.expected_fence)?;
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let auth = auth::admit(
            &tx,
            tenant,
            project,
            bearer,
            &request.read_set,
            Action::AccessManageProject,
            None,
        )
        .await?;
        let mut intent = Intent::load(&tx, tenant, project, &request.intent_id).await?;
        let original_set = intent.set.clone();
        intent.set = intent.effective_set(&tx, tenant, project, &auth).await?;
        if json!(intent.set) != json!(request.read_set) {
            return Err(PgError::Forbidden);
        }
        let (hash, replayed) = auth::replay(
            &tx,
            tenant,
            project,
            &auth,
            "delivery.pump.claim",
            &request.request_id,
            &request,
        )
        .await?;
        if let Some(result) = replayed {
            if result["data"]["state"] == "superseded" {
                tx.commit().await?;
                return Ok(None);
            }
            let lease = DeliverySyncLease {
                tenant: tenant.into(),
                project: project.into(),
                id: intent.id,
                set: request.read_set,
                worker: request.worker_id,
                credential_hash: crate::workstream_credential_hash(bearer)?,
                fence: version(
                    result["data"]["fence"]
                        .as_str()
                        .ok_or(PgError::SourceDivergence)?,
                )?,
            };
            require(&tx, tenant, project, &auth, &lease, None).await?;
            tx.commit().await?;
            return Ok(Some(lease));
        }
        if intent.fence != expected {
            return Err(PgError::StaleFence);
        }
        if intent.state == "leased" && intent.live {
            return Err(PgError::ResourceConflict);
        }
        if !matches!(intent.state.as_str(), "pending" | "leased" | "blocked") || !intent.due {
            return Err(PgError::PreconditionsChanged);
        }
        if !intent.current(&tx, tenant, project).await? {
            if intent.publication.is_some() {
                return Err(PgError::PreconditionsChanged);
            }
            tx.execute("UPDATE awr_team.delivery_sync_intents SET state='superseded',updated_at=clock_timestamp()
                WHERE tenant_id=$1 AND project_id=$2 AND id=$3", &[&tenant,&project,&intent.id]).await?;
            auth::finish(&tx,tenant,project,&auth,&request.read_set,"delivery.pump.claim",&request.request_id,&hash,
                json!({"intent_id":intent.id,"state":"superseded","fence":intent.fence.to_string()})).await?;
            tx.commit().await?;
            return Ok(None);
        }
        if json!(original_set) != json!(intent.set) {
            let changed = tx.execute("UPDATE awr_team.delivery_sync_intents SET read_set_json=$4,updated_at=clock_timestamp()
                WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND origin='domain_acceptance' AND publication_id IS NULL",
                &[&tenant,&project,&intent.id,&json!(intent.set)]).await?;
            if changed != 1 {
                return Err(PgError::PreconditionsChanged);
            }
        }
        if let Some(publication) = &intent.publication {
            let owner = tx
                .query_one(
                    "SELECT actor_id,client_id FROM awr_team.delivery_source_publications
                WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                    &[&tenant, &project, &publication],
                )
                .await?;
            if owner.get::<_, String>(0) != auth.actor_id
                || owner.get::<_, String>(1) != auth.client_id
            {
                return Err(PgError::Forbidden);
            }
        }
        let fence = intent.fence.checked_add(1).ok_or(PgError::StaleFence)?;
        tx.execute("UPDATE awr_team.delivery_sync_intents SET state='leased',fence=$4,worker_id=$5,actor_id=$6,
            client_id=$7,authority_binding=$8,expires_at=clock_timestamp()+make_interval(secs=>$9::integer),
            attempts=attempts+1,failure_code=NULL,updated_at=clock_timestamp()
            WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant,&project,&intent.id,&fence,&request.worker_id,&auth.actor_id,&auth.client_id,
              &auth::authority_binding(&auth,&request.read_set)?,&request.lease_seconds]).await?;
        let data = Intent::load(&tx, tenant, project, &intent.id)
            .await?
            .summary();
        auth::finish(
            &tx,
            tenant,
            project,
            &auth,
            &request.read_set,
            "delivery.pump.claim",
            &request.request_id,
            &hash,
            data,
        )
        .await?;
        let lease = DeliverySyncLease {
            tenant: tenant.into(),
            project: project.into(),
            id: intent.id,
            set: request.read_set,
            worker: request.worker_id,
            credential_hash: crate::workstream_credential_hash(bearer)?,
            fence,
        };
        require(&tx, tenant, project, &auth, &lease, None).await?;
        tx.commit().await?;
        Ok(Some(lease))
    }

    async fn leased_intent(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        lease: &DeliverySyncLease,
    ) -> PgResult<Intent> {
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let auth = auth::admit(
            &tx,
            tenant,
            project,
            bearer,
            &lease.set,
            Action::AccessManageProject,
            None,
        )
        .await?;
        require(&tx, tenant, project, &auth, lease, None).await?;
        let intent = Intent::load(&tx, tenant, project, &lease.id).await?;
        tx.commit().await?;
        Ok(intent)
    }

    /// Publish a refresh request, not an approval, execution grant or completion.
    pub async fn acknowledge_sync_refresh(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        lease: &DeliverySyncLease,
    ) -> PgResult<Value> {
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let auth = auth::admit(
            &tx,
            tenant,
            project,
            bearer,
            &lease.set,
            Action::AccessManageProject,
            None,
        )
        .await?;
        require(&tx, tenant, project, &auth, lease, None).await?;
        let intent = Intent::load(&tx, tenant, project, &lease.id).await?;
        if intent.kind != "refresh" {
            return Err(PgError::Forbidden);
        }
        let request = lease.request("ack");
        let data = json!({"intent_id":intent.id,"notification_id":intent.notification,
            "kind":"refresh","state":"succeeded","fence":lease.fence.to_string(),
            "refresh_available":true,"source_synchronized":false,"acceptance_ready":false});
        let hash = hash(&json!([request, lease.set, lease.worker, lease.fence]))?;
        // Recheck time inside this acknowledgement transaction after all reads.
        require(&tx, tenant, project, &auth, lease, None).await?;
        let changed = tx
            .execute(
                "UPDATE awr_team.delivery_notifications SET state='delivered'
            WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND state='pending'",
                &[&tenant, &project, &intent.notification],
            )
            .await?;
        if changed != 1 {
            return Err(PgError::PreconditionsChanged);
        }
        require(&tx, tenant, project, &auth, lease, None).await?;
        tx.execute(
            "UPDATE awr_team.delivery_sync_intents SET state='succeeded',result_json=$4,
            updated_at=clock_timestamp() WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &intent.id, &data],
        )
        .await?;
        let result = auth::finish(
            &tx,
            tenant,
            project,
            &auth,
            &lease.set,
            "delivery.pump.refresh",
            &request,
            &hash,
            data,
        )
        .await?;
        tx.commit().await?;
        Ok(result)
    }

    /// Link a durable publication before its first source-byte effect. Restart
    /// recovers that same journal, observing its actual bytes before any retry.
    pub async fn prepare_sync_source(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        lease: &DeliverySyncLease,
        lease_seconds_value: i32,
    ) -> PgResult<DeliveryPublicationStep> {
        lease_seconds(lease_seconds_value)?;
        let intent = self.leased_intent(tenant, project, bearer, lease).await?;
        if intent.kind != "source" {
            return Err(PgError::Forbidden);
        }
        let status = self
            .source_publication_status(tenant, project, bearer, &lease.set.work_id)
            .await?;
        let result = if let Some(id) = intent.publication {
            let publication = status["history"]
                .as_array()
                .and_then(|items| items.iter().find(|p| p["publication_id"] == id))
                .ok_or(PgError::SourceDivergence)?;
            let fence = publication["fence"]
                .as_str()
                .ok_or(PgError::SourceDivergence)?
                .to_owned();
            self.renew_source_publication_guarded(
                tenant,
                project,
                bearer,
                RenewDeliveryPublicationLease {
                    step: DeliveryPublicationStep {
                        request_id: lease.request(&format!("renew-{fence}")),
                        read_set: lease.set.clone(),
                        publication_id: id,
                        fence,
                    },
                    lease_seconds: lease_seconds_value,
                },
                lease,
            )
            .await?
        } else {
            self.prepare_source_publication_guarded(
                tenant,
                project,
                bearer,
                PrepareDeliverySourcePublication {
                    request_id: lease.request("prepare"),
                    read_set: lease.set.clone(),
                    candidate_digest: intent.candidate,
                    expected_selection_version: intent.selection.to_string(),
                    expected_metadata_revision: status["metadata_revision"]
                        .as_str()
                        .ok_or(PgError::SourceDivergence)?
                        .into(),
                    expected_source_fingerprint: status["confirmed_fingerprint"]
                        .as_str()
                        .ok_or(PgError::SourceDivergence)?
                        .into(),
                    completion_receipt_id: intent.completion,
                    lease_seconds: lease_seconds_value,
                },
                lease,
            )
            .await?
        };
        Ok(DeliveryPublicationStep {
            request_id: lease.request("prepared"),
            read_set: lease.set.clone(),
            publication_id: result["data"]["publication_id"]
                .as_str()
                .ok_or(PgError::SourceDivergence)?
                .into(),
            fence: result["data"]["fence"]
                .as_str()
                .ok_or(PgError::SourceDivergence)?
                .into(),
        })
    }

    async fn sync_source_step(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        lease: &DeliverySyncLease,
        confirm: bool,
    ) -> PgResult<Value> {
        let intent = self.leased_intent(tenant, project, bearer, lease).await?;
        if intent.kind != "source" {
            return Err(PgError::Forbidden);
        }
        let id = intent.publication.ok_or(PgError::InactiveCandidate)?;
        // A read-only physical observation precedes recovery; the publisher
        // repeats it under its bound source handle and atomic worker guard.
        let status = self
            .source_publication_status(tenant, project, bearer, &lease.set.work_id)
            .await?;
        let publication = status["history"]
            .as_array()
            .and_then(|items| items.iter().find(|p| p["publication_id"] == id))
            .ok_or(PgError::SourceDivergence)?;
        let fence = publication["fence"]
            .as_str()
            .ok_or(PgError::SourceDivergence)?
            .to_owned();
        let request = DeliveryPublicationStep {
            request_id: lease.request(&format!(
                "{}-{fence}",
                if confirm { "confirm" } else { "write" }
            )),
            read_set: lease.set.clone(),
            publication_id: id,
            fence,
        };
        self.publication_step_guarded(tenant, project, bearer, request, confirm, lease)
            .await
    }

    pub async fn write_sync_source(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        lease: &DeliverySyncLease,
    ) -> PgResult<Value> {
        self.sync_source_step(tenant, project, bearer, lease, false)
            .await
    }

    pub async fn confirm_sync_source(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        lease: &DeliverySyncLease,
    ) -> PgResult<Value> {
        self.sync_source_step(tenant, project, bearer, lease, true)
            .await
    }

    /// One bounded dispatch. Server scheduling and cancellation are separate.
    pub async fn process_sync_intent(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        lease: &DeliverySyncLease,
        publication_lease_seconds: i32,
    ) -> PgResult<Value> {
        let intent = self.leased_intent(tenant, project, bearer, lease).await?;
        if intent.kind == "refresh" {
            return self
                .acknowledge_sync_refresh(tenant, project, bearer, lease)
                .await;
        }
        self.prepare_sync_source(tenant, project, bearer, lease, publication_lease_seconds)
            .await?;
        let written = self
            .write_sync_source(tenant, project, bearer, lease)
            .await?;
        if written["data"]["phase"] != "source_written" {
            return Ok(written);
        }
        self.confirm_sync_source(tenant, project, bearer, lease)
            .await
    }

    /// Defer an observed failure with a bounded code, never raw error/secret text.
    pub async fn defer_sync_intent(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        lease: &DeliverySyncLease,
        failure_code: &str,
        retry_seconds: i32,
    ) -> PgResult<Value> {
        if !(1..=3600).contains(&retry_seconds)
            || !matches!(
                failure_code,
                "source_conflict"
                    | "source_unavailable"
                    | "source_failed"
                    | "preconditions_changed"
            )
        {
            return Err(invalid());
        }
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let auth = auth::admit(
            &tx,
            tenant,
            project,
            bearer,
            &lease.set,
            Action::AccessManageProject,
            None,
        )
        .await?;
        require(&tx, tenant, project, &auth, lease, None).await?;
        let data = json!({"intent_id":lease.id,"state":"blocked","failure_code":failure_code,
            "retry_seconds":retry_seconds,"fence":lease.fence.to_string(),"acceptance_ready":false});
        let request = lease.request("defer");
        let hash = hash(&json!([request, data]))?;
        tx.execute("UPDATE awr_team.delivery_sync_intents SET state='blocked',failure_code=$4,
            retry_at=clock_timestamp()+make_interval(secs=>$5::integer),updated_at=clock_timestamp(),result_json=$6
            WHERE tenant_id=$1 AND project_id=$2 AND id=$3", &[&tenant,&project,&lease.id,&failure_code,&retry_seconds,&data]).await?;
        let result = auth::finish(
            &tx,
            tenant,
            project,
            &auth,
            &lease.set,
            "delivery.pump.defer",
            &request,
            &hash,
            data,
        )
        .await?;
        tx.commit().await?;
        Ok(result)
    }
}
