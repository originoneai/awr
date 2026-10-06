//! Durable target ownership survives lost replies and service reconstruction.
use super::*;

struct Intent {
    prepare: PrepareDeliveryIntegration,
    request: IntegrationRequest,
    eligibility_digest: String,
    issuer: CredentialReference,
    issuer_binding: String,
    state: String,
    lease_id: Option<String>,
    lease_actor: Option<String>,
    lease_client: Option<String>,
    lease_binding: Option<String>,
    lease_live: bool,
    dispatch_receipt: Option<Value>,
}

// Common auth decides permission. Pin the exact admitted delegation and parent
// records too; unchanged effective actions do not preserve queued approval.
async fn authority_binding(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    set: &DeliveryReadSet,
) -> PgResult<String> {
    let mut chain = Vec::new();
    let mut id = auth.delegation_id.clone();
    while let Some(current) = id {
        if chain.len() >= 32 {
            return Err(PgError::Forbidden);
        }
        let row = tx
            .query_opt(
                "SELECT body_json,status,revoked_at_ms FROM awr_team.agent_authorizations
            WHERE tenant_id=$1 AND project_id=$2 AND id=$3 FOR SHARE",
                &[&tenant, &project, &current],
            )
            .await?
            .ok_or(PgError::Forbidden)?;
        let body: Value = row.get(0);
        let grant: awr_core::AgentAuthorization =
            serde_json::from_value(body.clone()).map_err(|_| PgError::Forbidden)?;
        id = grant.parent_authorization_id;
        chain.push(json!({"id":current,"body":body,"status":row.get::<_,String>(1),"revoked_at_ms":row.get::<_,Option<i64>>(2)}));
    }
    hash(&json!({"authority":auth::authority_binding(auth,set)?,"delegation_chain":chain}))
}

async fn worker_binding(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    set: &DeliveryReadSet,
    bearer: &str,
) -> PgResult<String> {
    hash(
        &json!({"authority":authority_binding(tx,tenant,project,auth,set).await?,
        "credential":crate::workstream_auth::workstream_credential_hash(bearer)?}),
    )
}

async fn load(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    id: &str,
    work: &str,
) -> PgResult<Intent> {
    identity(id)?;
    let row = tx.query_opt(
        "SELECT request_json,eligibility_digest,issuer_credential_id,issuer_secret_hash,issuer_authority_binding,
                state,lease_id,lease_actor_id,lease_client_id,lease_authority_binding,
                COALESCE(lease_expires_at>clock_timestamp(),false),dispatch_receipt_json
         FROM awr_team.delivery_integration_intents WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND work_id=$4 FOR UPDATE",
        &[&tenant,&project,&id,&work],
    ).await?.ok_or(PgError::Forbidden)?;
    let value: Value = row.get(0);
    Ok(Intent {
        prepare: serde_json::from_value(value["prepare"].clone())
            .map_err(|_| PgError::SourceDivergence)?,
        request: serde_json::from_value(value["request"].clone())
            .map_err(|_| PgError::SourceDivergence)?,
        eligibility_digest: row.get(1),
        issuer: CredentialReference {
            credential_id: row.get(2),
            secret_hash: row.get(3),
        },
        issuer_binding: row.get(4),
        state: row.get(5),
        lease_id: row.get(6),
        lease_actor: row.get(7),
        lease_client: row.get(8),
        lease_binding: row.get(9),
        lease_live: row.get(10),
        dispatch_receipt: row.get(11),
    })
}

async fn revalidate(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    intent: &Intent,
) -> PgResult<Eligibility> {
    let set = &intent.prepare.read_set;
    let issuer = auth::admit_reference(
        tx,
        tenant,
        project,
        &intent.issuer,
        set,
        Action::DeliveryFinalize,
        None,
    )
    .await?;
    if authority_binding(tx, tenant, project, &issuer, set).await? != intent.issuer_binding {
        return Err(PgError::Forbidden);
    }
    let eligibility = eligibility::resolve(tx, tenant, project, &issuer, &intent.prepare).await?;
    if hash(&eligibility.binding)? != intent.eligibility_digest {
        return Err(PgError::PreconditionsChanged);
    }
    Ok(eligibility)
}

async fn worker(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    set: &DeliveryReadSet,
    intent: &Intent,
) -> PgResult<()> {
    if auth.actor_kind != "system" {
        return Err(PgError::Forbidden);
    }
    let connector =
        connectors::load(tx, tenant, project, auth, set, &intent.prepare.connector_id).await?;
    if connector.source != FactSource::AdapterObservation
        || connector.version != version(&intent.prepare.connector_version)?
    {
        return Err(PgError::PreconditionsChanged);
    }
    Ok(())
}

async fn release_guard(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    id: &str,
) -> PgResult<()> {
    if tx.execute("DELETE FROM awr_team.delivery_integration_target_guards WHERE tenant_id=$1 AND project_id=$2 AND intent_id=$3",
        &[&tenant,&project,&id]).await? != 1 {
        return Err(PgError::SourceDivergence);
    }
    Ok(())
}

impl DeliverySyncStore {
    /// Prepare one exact approved candidate. This reserves a target, not an effect.
    pub async fn prepare_integration(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        request: PrepareDeliveryIntegration,
    ) -> PgResult<Value> {
        bounded(&request)?;
        for id in [
            &request.connector_id,
            &request.evidence_id,
            &request.review_round_id,
            &request.review_decision_id,
        ] {
            identity(id)?;
        }
        digest(&request.candidate_digest)?;
        version(&request.selection_version)?;
        version(&request.connector_version)?;
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let set = &request.read_set;
        let auth = auth::admit(
            &tx,
            tenant,
            project,
            bearer,
            set,
            Action::DeliveryFinalize,
            None,
        )
        .await?;
        let op = "delivery.integration.prepare";
        let (request_hash, replay) = auth::replay(
            &tx,
            tenant,
            project,
            &auth,
            op,
            &request.request_id,
            &request,
        )
        .await?;
        if let Some(receipt) = replay {
            tx.commit().await?;
            return Ok(receipt);
        }
        let eligibility = eligibility::resolve(&tx, tenant, project, &auth, &request).await?;
        let id = crate::tx::new_id();
        let neutral = IntegrationRequest {
            binding: eligibility.candidate.binding.clone(),
            request_id: awr_team::RequestId::new(&id).map_err(|_| invalid())?,
            operation: request.operation.clone(),
            review_round_id: request.review_round_id.clone(),
            review_decision_id: request.review_decision_id.clone(),
            verified_checks: eligibility.checks,
        };
        DeliveryRecord::IntegrationRequest(neutral.clone())
            .validate()
            .map_err(|_| invalid())?;
        let eligibility_digest = hash(&eligibility.binding)?;
        let reference = credential_reference(bearer)?;
        tx.execute("INSERT INTO awr_team.delivery_integration_intents(tenant_id,project_id,id,work_id,connector_id,
            request_json,eligibility_json,eligibility_digest,issuer_credential_id,issuer_secret_hash,issuer_authority_binding,state)
            VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,'prepared')",
            &[&tenant,&project,&id,&set.work_id,&request.connector_id,&json!({"prepare":request,"request":neutral}),
              &eligibility.binding,&eligibility_digest,&reference.credential_id,&reference.secret_hash,&authority_binding(&tx,tenant,project,&auth,set).await?]).await?;
        let target = &eligibility.candidate.binding.target;
        if tx.execute("INSERT INTO awr_team.delivery_integration_target_guards(tenant_id,resource,reference,project_id,intent_id)
            VALUES($1,$2,$3,$4,$5) ON CONFLICT(tenant_id,resource,reference) DO NOTHING",
            &[&tenant,&target.resource,&target.reference,&project,&id]).await? != 1 {
            return Err(PgError::RecoveryBlocked);
        }
        let receipt = auth::finish(&tx,tenant,project,&auth,set,op,&request.request_id,&request_hash,
            json!({"integration_id":id,"state":"prepared","candidate_digest":request.candidate_digest,
                "eligibility_digest":eligibility_digest,"integration_request":neutral})).await?;
        tx.commit().await?;
        Ok(receipt)
    }

    /// Only an independent configured system principal leases the durable intent.
    pub async fn lease_integration(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        request: LeaseDeliveryIntegration,
    ) -> PgResult<Value> {
        bounded(&request)?;
        if !(5..=300).contains(&request.lease_seconds) {
            return Err(invalid());
        }
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let set = &request.read_set;
        let auth = auth::admit(
            &tx,
            tenant,
            project,
            bearer,
            set,
            Action::DeliveryFinalize,
            None,
        )
        .await?;
        let op = "delivery.integration.lease";
        let (request_hash, replay) = auth::replay(
            &tx,
            tenant,
            project,
            &auth,
            op,
            &request.request_id,
            &request,
        )
        .await?;
        if let Some(receipt) = replay {
            tx.commit().await?;
            return Ok(receipt);
        }
        let intent = load(&tx, tenant, project, &request.integration_id, &set.work_id).await?;
        worker(&tx, tenant, project, &auth, set, &intent).await?;
        if !matches!(intent.state.as_str(), "prepared" | "leased") {
            return Err(PgError::RecoveryBlocked);
        }
        if intent.state == "leased" && intent.lease_live {
            return Err(PgError::ClaimHeld);
        }
        revalidate(&tx, tenant, project, &intent).await?;
        let lease_id = crate::tx::new_id();
        let row = tx.query_one("UPDATE awr_team.delivery_integration_intents SET state='leased',lease_id=$4,
            lease_actor_id=$5,lease_client_id=$6,lease_authority_binding=$7,lease_expires_at=clock_timestamp()+make_interval(secs=>$8::integer)
            WHERE tenant_id=$1 AND project_id=$2 AND id=$3 RETURNING lease_expires_at::text",
            &[&tenant,&project,&request.integration_id,&lease_id,&auth.actor_id,&auth.client_id,&worker_binding(&tx,tenant,project,&auth,set,bearer).await?,&request.lease_seconds]).await?;
        let receipt = auth::finish(&tx,tenant,project,&auth,set,op,&request.request_id,&request_hash,
            json!({"integration_id":request.integration_id,"state":"leased","lease_id":lease_id,"expires_at":row.get::<_,String>(0)})).await?;
        tx.commit().await?;
        Ok(receipt)
    }

    /// Persist dispatch before returning its sole in-memory effect capability.
    pub async fn dispatch_integration(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        request: DispatchDeliveryIntegration,
    ) -> PgResult<DeliveryIntegrationDispatch> {
        bounded(&request)?;
        identity(&request.lease_id)?;
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let set = &request.read_set;
        let auth = auth::admit(
            &tx,
            tenant,
            project,
            bearer,
            set,
            Action::DeliveryFinalize,
            None,
        )
        .await?;
        let op = "delivery.integration.dispatch";
        let (request_hash, replay) = auth::replay(
            &tx,
            tenant,
            project,
            &auth,
            op,
            &request.request_id,
            &request,
        )
        .await?;
        if let Some(receipt) = replay {
            tx.commit().await?;
            return Ok(DeliveryIntegrationDispatch {
                receipt,
                permit: None,
            });
        }
        let intent = load(&tx, tenant, project, &request.integration_id, &set.work_id).await?;
        worker(&tx, tenant, project, &auth, set, &intent).await?;
        if intent.lease_id.as_deref() != Some(request.lease_id.as_str()) {
            return Err(PgError::StaleFence);
        }
        if intent.lease_actor.as_deref() != Some(auth.actor_id.as_str())
            || intent.lease_client.as_deref() != Some(auth.client_id.as_str())
        {
            return Err(PgError::Forbidden);
        }
        if matches!(
            intent.state.as_str(),
            "dispatched" | "unknown" | "confirmed"
        ) {
            let receipt = intent.dispatch_receipt.ok_or(PgError::SourceDivergence)?;
            tx.commit().await?;
            return Ok(DeliveryIntegrationDispatch {
                receipt,
                permit: None,
            });
        }
        if intent.state != "leased" {
            return Err(PgError::RecoveryBlocked);
        }
        if !intent.lease_live {
            return Err(PgError::LeaseExpired);
        }
        if intent.lease_binding.as_deref()
            != Some(
                worker_binding(&tx, tenant, project, &auth, set, bearer)
                    .await?
                    .as_str(),
            )
        {
            return Err(PgError::Forbidden);
        }
        let eligibility = revalidate(&tx, tenant, project, &intent).await?;
        let receipt = auth::finish(&tx,tenant,project,&auth,set,op,&request.request_id,&request_hash,
            json!({"integration_id":request.integration_id,"state":"dispatched","lease_id":request.lease_id,
                "candidate_digest":intent.prepare.candidate_digest,"eligibility_digest":intent.eligibility_digest,
                "integration_request":intent.request})).await?;
        // Time can advance while resolving artifacts or persisting the receipt.
        // Reuse live auth at the dispatch boundary, then atomically check the
        // lease deadline. Any failure rolls back the receipt and audit too.
        let live_worker = auth::admit(
            &tx,
            tenant,
            project,
            bearer,
            set,
            Action::DeliveryFinalize,
            None,
        )
        .await?;
        let live_issuer = auth::admit_reference(
            &tx,
            tenant,
            project,
            &intent.issuer,
            &intent.prepare.read_set,
            Action::DeliveryFinalize,
            None,
        )
        .await?;
        if worker_binding(&tx, tenant, project, &live_worker, set, bearer)
            .await?
            .as_str()
            != intent.lease_binding.as_deref().ok_or(PgError::Forbidden)?
            || authority_binding(&tx, tenant, project, &live_issuer, &intent.prepare.read_set)
                .await?
                != intent.issuer_binding
        {
            return Err(PgError::Forbidden);
        }
        if tx.execute("UPDATE awr_team.delivery_integration_intents SET state='dispatched',dispatched_at=clock_timestamp(),dispatch_receipt_json=$4
            WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND lease_id=$5 AND lease_expires_at>clock_timestamp()",
            &[&tenant,&project,&request.integration_id,&receipt,&request.lease_id]).await? != 1 {
            return Err(PgError::LeaseExpired);
        }
        let permit = DeliveryIntegrationPermit {
            candidate: eligibility.candidate,
            request: intent.request,
            read_set: intent.prepare.read_set,
            connector_id: intent.prepare.connector_id,
            connector_version: intent.prepare.connector_version,
            eligibility_digest: intent.eligibility_digest,
        };
        tx.commit().await?;
        Ok(DeliveryIntegrationDispatch {
            receipt,
            permit: Some(permit),
        })
    }

    /// A bound neutral adapter fact resolves an effect, never completes the task.
    pub async fn confirm_integration(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        request: ConfirmDeliveryIntegration,
    ) -> PgResult<Value> {
        bounded(&request)?;
        identity(&request.fact_id)?;
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let set = &request.read_set;
        let auth = auth::admit(
            &tx,
            tenant,
            project,
            bearer,
            set,
            Action::DeliveryFinalize,
            None,
        )
        .await?;
        let op = "delivery.integration.confirm";
        let (request_hash, replay) = auth::replay(
            &tx,
            tenant,
            project,
            &auth,
            op,
            &request.request_id,
            &request,
        )
        .await?;
        if let Some(receipt) = replay {
            tx.commit().await?;
            return Ok(receipt);
        }
        let intent = load(&tx, tenant, project, &request.integration_id, &set.work_id).await?;
        // Current mapped identity may record an old effect after issuer/source
        // changes; the original binding remains immutable and historical.
        if auth.actor_kind != "system" {
            return Err(PgError::Forbidden);
        }
        let connector = connectors::load(
            &tx,
            tenant,
            project,
            &auth,
            set,
            &intent.prepare.connector_id,
        )
        .await?;
        if connector.source != FactSource::AdapterObservation {
            return Err(PgError::Forbidden);
        }
        if !matches!(intent.state.as_str(), "dispatched" | "unknown") {
            return Err(PgError::RecoveryBlocked);
        }
        let row = tx.query_opt("SELECT f.envelope_json,i.connector_id,x.binding_digest,x.connector_version
            FROM awr_team.delivery_facts f JOIN awr_team.delivery_inbox i ON i.tenant_id=f.tenant_id AND i.project_id=f.project_id AND i.id=f.inbox_id
            JOIN awr_team.delivery_inspections x ON x.tenant_id=i.tenant_id AND x.project_id=i.project_id AND x.id=i.inspection_id
            JOIN awr_team.delivery_integration_intents d ON d.tenant_id=f.tenant_id AND d.project_id=f.project_id AND d.id=$4
            WHERE f.tenant_id=$1 AND f.project_id=$2 AND f.id=$3 AND x.created_at>=d.dispatched_at AND i.recorded_at>=d.dispatched_at",
            &[&tenant,&project,&request.fact_id,&request.integration_id]).await?.ok_or(PgError::EvidenceInvalid)?;
        let envelope: DeliveryEnvelope =
            serde_json::from_value(row.get(0)).map_err(|_| PgError::SourceDivergence)?;
        envelope.validate().map_err(|_| PgError::EvidenceInvalid)?;
        envelope
            .record
            .validate_against(&intent.request.binding)
            .map_err(|_| PgError::BindingInvalid)?;
        let DeliveryRecord::IntegrationObservation(observation) = &envelope.record else {
            return Err(PgError::EvidenceInvalid);
        };
        if row.get::<_, String>(1) != intent.prepare.connector_id
            || row.get::<_, String>(2) != intent.prepare.candidate_digest
            || observation.request_id.as_ref().map(|r| r.as_str())
                != Some(request.integration_id.as_str())
            || observation.provenance.source != FactSource::AdapterObservation
        {
            return Err(PgError::EvidenceInvalid);
        }
        let current = match revalidate(&tx, tenant, project, &intent).await {
            Ok(_) => {
                connector.version == version(&intent.prepare.connector_version)?
                    && row.get::<_, i64>(3) == connector.version
            }
            Err(PgError::Db(e)) => return Err(PgError::Db(e)),
            Err(PgError::Pool(e)) => return Err(PgError::Pool(e)),
            Err(_) => false,
        };
        let (state, terminal) = match observation.outcome {
            IntegrationOutcome::Applied => ("confirmed", true),
            IntegrationOutcome::Rejected => ("rejected", true),
            IntegrationOutcome::Unknown => ("unknown", false),
            IntegrationOutcome::Pending => ("dispatched", false),
        };
        tx.execute("UPDATE awr_team.delivery_integration_intents SET state=$4,confirmation_fact_id=$5,confirmation_current=$6
            WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant,&project,&request.integration_id,&state,&request.fact_id,&current]).await?;
        if terminal {
            release_guard(&tx, tenant, project, &request.integration_id).await?;
        }
        let receipt = auth::finish(&tx,tenant,project,&auth,set,op,&request.request_id,&request_hash,
            json!({"integration_id":request.integration_id,"state":state,"fact_id":request.fact_id,
                "current":current,"guard_released":terminal,"candidate_digest":intent.prepare.candidate_digest})).await?;
        tx.commit().await?;
        Ok(receipt)
    }

    /// Cancellation before dispatch is safe; uncertain effects cannot be cancelled here.
    pub async fn reject_prepared_integration(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        request: RejectPreparedDeliveryIntegration,
    ) -> PgResult<Value> {
        bounded(&request)?;
        text(&request.reason, 1024)?;
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let set = &request.read_set;
        let auth = auth::admit(
            &tx,
            tenant,
            project,
            bearer,
            set,
            Action::DeliveryFinalize,
            None,
        )
        .await?;
        let op = "delivery.integration.reject_prepared";
        let (request_hash, replay) = auth::replay(
            &tx,
            tenant,
            project,
            &auth,
            op,
            &request.request_id,
            &request,
        )
        .await?;
        if let Some(receipt) = replay {
            tx.commit().await?;
            return Ok(receipt);
        }
        let intent = load(&tx, tenant, project, &request.integration_id, &set.work_id).await?;
        let reference = credential_reference(bearer)?;
        if reference.credential_id != intent.issuer.credential_id
            || reference.secret_hash != intent.issuer.secret_hash
        {
            worker(&tx, tenant, project, &auth, set, &intent).await?;
        }
        if !matches!(intent.state.as_str(), "prepared" | "leased") {
            return Err(PgError::RecoveryBlocked);
        }
        tx.execute("UPDATE awr_team.delivery_integration_intents SET state='rejected' WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant,&project,&request.integration_id]).await?;
        release_guard(&tx, tenant, project, &request.integration_id).await?;
        let receipt = auth::finish(&tx,tenant,project,&auth,set,op,&request.request_id,&request_hash,
            json!({"integration_id":request.integration_id,"state":"rejected","before_dispatch":true,"reason":request.reason,"guard_released":true})).await?;
        tx.commit().await?;
        Ok(receipt)
    }

    /// Receipts are inspectable after reconstruction; private authority is omitted.
    pub async fn inspect_integration(
        &self,
        tenant: &str,
        project: &str,
        bearer: &str,
        work: &str,
        id: &str,
    ) -> PgResult<Value> {
        identity(work)?;
        identity(id)?;
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
            now_ms(),
        )
        .await?;
        let (binding, _) =
            crate::workstream_read::work_binding(&tx, tenant, project, &auth, work).await?;
        crate::workstream_auth::authorize_domain_action(
            &auth,
            Action::WorkRead,
            Some(binding.workstream_id),
            Some(work),
        )?;
        let row = tx.query_opt("SELECT state,request_json,eligibility_digest,lease_id,lease_expires_at::text,dispatched_at::text,
            dispatch_receipt_json,confirmation_fact_id,confirmation_current FROM awr_team.delivery_integration_intents
            WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND work_id=$4",
            &[&tenant,&project,&id,&work]).await?.ok_or(PgError::Forbidden)?;
        let stored: Value = row.get(1);
        let receipt = json!({"integration_id":id,"state":row.get::<_,String>(0),"integration_request":stored["request"],
            "eligibility_digest":row.get::<_,String>(2),"lease_id":row.get::<_,Option<String>>(3),"lease_expires_at":row.get::<_,Option<String>>(4),
            "dispatched_at":row.get::<_,Option<String>>(5),"dispatch_receipt":row.get::<_,Option<Value>>(6),
            "confirmation_fact_id":row.get::<_,Option<String>>(7),"confirmation_current":row.get::<_,Option<bool>>(8),
            "execution_authorized":false,"acceptance_ready":false,"source_synchronized":false});
        tx.commit().await?;
        Ok(receipt)
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
