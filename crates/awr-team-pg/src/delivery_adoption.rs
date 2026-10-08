//! Team PG persistence for versioned delivery dependencies and adoption credentials (WS-030).
use crate::error::{PgError, PgResult};
use crate::tx::{bind_workstream_scope, lock_active_project, new_id};
use awr_core::{
    AdoptDeliveryRequest, AdoptionCredential, CompletionAcceptanceProof, DeliveryAvailability,
    DeliveryCredentialReceipt, DeliveryError, DeliveryVersion, EvidenceLevel, ExportAuthorization,
    GrantExportAuthorizationRequest, HardDeliveryDependency, Id, RegisterHardDependencyRequest,
    RevokeExportAuthorizationRequest, RevokeHardDependencyRequest,
    adopt_delivery_credential as core_adopt_delivery_credential, apply_export_revoke,
    apply_hard_dependency_revoke, validate_export_grant, validate_hard_dependency_registration,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio_postgres::Transaction;

fn map_delivery(err: DeliveryError) -> PgError {
    match err {
        DeliveryError::InvalidDefinition => PgError::Protocol("invalid delivery definition".into()),
        DeliveryError::BindingMismatch => PgError::Protocol("delivery binding mismatch".into()),
        DeliveryError::InvalidTime => PgError::Protocol("invalid delivery time".into()),
        DeliveryError::NotSatisfied(a) => PgError::Protocol(format!(
            "delivery not satisfied: {:?}/{:?}",
            a.status, a.reason
        )),
    }
}

fn policy_str(p: awr_core::DeliveryVersionPolicy) -> &'static str {
    match p {
        awr_core::DeliveryVersionPolicy::FixedDelivery => "fixed_delivery",
        awr_core::DeliveryVersionPolicy::CurrentContract => "current_contract",
    }
}

fn dep_status_str(s: awr_core::HardDependencyStatus) -> &'static str {
    match s {
        awr_core::HardDependencyStatus::Active => "active",
        awr_core::HardDependencyStatus::Revoked => "revoked",
    }
}

fn export_status_str(s: awr_core::ExportAuthorizationStatus) -> &'static str {
    match s {
        awr_core::ExportAuthorizationStatus::Granted => "granted",
        awr_core::ExportAuthorizationStatus::Denied => "denied",
        awr_core::ExportAuthorizationStatus::Revoked => "revoked",
    }
}

fn cred_status_str(s: awr_core::AdoptionCredentialStatus) -> &'static str {
    match s {
        awr_core::AdoptionCredentialStatus::Active => "active",
        awr_core::AdoptionCredentialStatus::Stale => "stale",
        awr_core::AdoptionCredentialStatus::Revoked => "revoked",
    }
}

pub struct DeliveryAdoptionStore {
    pool: crate::PgPool,
}

impl DeliveryAdoptionStore {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            pool: crate::PgPool::new(url),
        }
    }

    pub fn from_config(config: tokio_postgres::Config) -> Self {
        Self {
            pool: crate::PgPool::from_config(config),
        }
    }

    async fn connect(&self) -> PgResult<crate::PgClient> {
        self.pool.get().await
    }

    pub async fn get_dependency(
        &self,
        tenant: &str,
        project: &str,
        dependency_id: &str,
    ) -> PgResult<Option<HardDeliveryDependency>> {
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_workstream_scope(&tx, tenant, project).await?;
        let out = load_dep(&tx, tenant, project, dependency_id).await?;
        tx.commit().await?;
        Ok(out)
    }

    pub async fn register_dependency(
        &self,
        tenant: &str,
        project: &str,
        req: &RegisterHardDependencyRequest,
    ) -> PgResult<(HardDeliveryDependency, DeliveryCredentialReceipt)> {
        let request_hash = request_digest(tenant, project, "register_dependency", req)?;
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_workstream_scope(&tx, tenant, project).await?;
        lock_active_project(&tx, tenant, project).await?;
        if let Some(outcome) = load_receipt(
            &tx,
            tenant,
            project,
            &req.request_key,
            "register_dependency",
            &request_hash,
        )
        .await?
        {
            tx.commit().await?;
            return Ok(outcome);
        }
        if project != req.provider.project_id || project != req.consumer.project_id {
            return Err(PgError::Protocol(
                "delivery dependency project must match tenant project".into(),
            ));
        }
        if load_dep(&tx, tenant, project, &req.dependency_id)
            .await?
            .is_some()
        {
            return Err(PgError::Protocol(
                "hard delivery dependency id already registered".into(),
            ));
        }
        let dep = validate_hard_dependency_registration(req).map_err(map_delivery)?;
        persist_dep(&tx, tenant, project, &dep).await?;
        let receipt = record_receipt(
            &tx,
            tenant,
            project,
            &req.request_key,
            "register_dependency",
            &request_hash,
            &dep,
        )
        .await?;
        tx.commit().await?;
        Ok((dep, receipt))
    }

    pub async fn revoke_dependency(
        &self,
        tenant: &str,
        project: &str,
        req: &RevokeHardDependencyRequest,
    ) -> PgResult<(HardDeliveryDependency, DeliveryCredentialReceipt)> {
        let request_hash = request_digest(tenant, project, "revoke_dependency", req)?;
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_workstream_scope(&tx, tenant, project).await?;
        lock_active_project(&tx, tenant, project).await?;
        if let Some(outcome) = load_receipt(
            &tx,
            tenant,
            project,
            &req.request_key,
            "revoke_dependency",
            &request_hash,
        )
        .await?
        {
            tx.commit().await?;
            return Ok(outcome);
        }
        let current = load_dep(&tx, tenant, project, &req.dependency_id)
            .await?
            .ok_or_else(|| PgError::Protocol("hard delivery dependency missing".into()))?;
        let dep = apply_hard_dependency_revoke(&current, req).map_err(map_delivery)?;
        persist_dep(&tx, tenant, project, &dep).await?;
        let receipt = record_receipt(
            &tx,
            tenant,
            project,
            &req.request_key,
            "revoke_dependency",
            &request_hash,
            &dep,
        )
        .await?;
        tx.commit().await?;
        Ok((dep, receipt))
    }

    pub async fn get_export(
        &self,
        tenant: &str,
        project: &str,
        authorization_id: &str,
    ) -> PgResult<Option<ExportAuthorization>> {
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_workstream_scope(&tx, tenant, project).await?;
        let out = load_export(&tx, tenant, project, authorization_id).await?;
        tx.commit().await?;
        Ok(out)
    }

    pub async fn grant_export(
        &self,
        tenant: &str,
        project: &str,
        req: &GrantExportAuthorizationRequest,
    ) -> PgResult<(ExportAuthorization, DeliveryCredentialReceipt)> {
        let request_hash = request_digest(tenant, project, "grant_export", req)?;
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_workstream_scope(&tx, tenant, project).await?;
        lock_active_project(&tx, tenant, project).await?;
        if let Some(outcome) = load_receipt(
            &tx,
            tenant,
            project,
            &req.request_key,
            "grant_export",
            &request_hash,
        )
        .await?
        {
            tx.commit().await?;
            return Ok(outcome);
        }
        if project != req.project_id {
            return Err(PgError::Protocol(
                "export authorization project must match tenant project".into(),
            ));
        }
        if load_export(&tx, tenant, project, &req.authorization_id)
            .await?
            .is_some()
        {
            return Err(PgError::Protocol(
                "export authorization id already registered".into(),
            ));
        }
        let auth = validate_export_grant(req).map_err(map_delivery)?;
        persist_export(&tx, tenant, project, &auth).await?;
        let receipt = record_receipt(
            &tx,
            tenant,
            project,
            &req.request_key,
            "grant_export",
            &request_hash,
            &auth,
        )
        .await?;
        tx.commit().await?;
        Ok((auth, receipt))
    }

    pub async fn revoke_export(
        &self,
        tenant: &str,
        project: &str,
        req: &RevokeExportAuthorizationRequest,
    ) -> PgResult<(ExportAuthorization, DeliveryCredentialReceipt)> {
        let request_hash = request_digest(tenant, project, "revoke_export", req)?;
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_workstream_scope(&tx, tenant, project).await?;
        lock_active_project(&tx, tenant, project).await?;
        if let Some(outcome) = load_receipt(
            &tx,
            tenant,
            project,
            &req.request_key,
            "revoke_export",
            &request_hash,
        )
        .await?
        {
            tx.commit().await?;
            return Ok(outcome);
        }
        let current = load_export(&tx, tenant, project, &req.authorization_id)
            .await?
            .ok_or_else(|| PgError::Protocol("export authorization missing".into()))?;
        let auth = apply_export_revoke(&current, req).map_err(map_delivery)?;
        persist_export(&tx, tenant, project, &auth).await?;
        let receipt = record_receipt(
            &tx,
            tenant,
            project,
            &req.request_key,
            "revoke_export",
            &request_hash,
            &auth,
        )
        .await?;
        tx.commit().await?;
        Ok((auth, receipt))
    }

    pub async fn get_credential(
        &self,
        tenant: &str,
        project: &str,
        credential_id: &str,
    ) -> PgResult<Option<AdoptionCredential>> {
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_workstream_scope(&tx, tenant, project).await?;
        let out = load_credential(&tx, tenant, project, credential_id).await?;
        tx.commit().await?;
        Ok(out)
    }

    pub async fn adopt(
        &self,
        tenant: &str,
        project: &str,
        req: &AdoptDeliveryRequest,
    ) -> PgResult<(AdoptionCredential, DeliveryCredentialReceipt)> {
        let request_hash = request_digest(tenant, project, "adopt", req)?;
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_workstream_scope(&tx, tenant, project).await?;
        lock_active_project(&tx, tenant, project).await?;
        if let Some(outcome) = load_receipt(
            &tx,
            tenant,
            project,
            &req.request_key,
            "adopt",
            &request_hash,
        )
        .await?
        {
            tx.commit().await?;
            return Ok(outcome);
        }
        if project != req.dependency.provider.project_id
            || project != req.dependency.consumer.project_id
        {
            return Err(PgError::Protocol(
                "adoption credential project must match tenant project".into(),
            ));
        }
        let stored_dep = load_dep(&tx, tenant, project, &req.dependency.id)
            .await?
            .ok_or_else(|| PgError::Protocol("hard delivery dependency missing".into()))?;
        if stored_dep != req.dependency {
            return Err(PgError::Protocol(
                "adoption dependency snapshot does not match store".into(),
            ));
        }
        let stored_export = load_export(&tx, tenant, project, &req.export_authorization.id)
            .await?
            .ok_or_else(|| PgError::Protocol("export authorization missing".into()))?;
        if stored_export != req.export_authorization {
            return Err(PgError::Protocol(
                "adoption export authorization snapshot does not match store".into(),
            ));
        }
        if load_credential(&tx, tenant, project, &req.credential_id)
            .await?
            .is_some()
        {
            return Err(PgError::Protocol(
                "adoption credential id already registered".into(),
            ));
        }
        // WS-018 completion is authoritative. Request-supplied proof fields are
        // never trusted on their own. fixed_delivery may adopt a historical
        // receipt after the current selection moves; current_contract revalidates
        // against the live selected completion.
        let trusted_completion = load_trusted_completion_proof(
            &tx,
            tenant,
            project,
            &stored_dep.provider.work_item_id,
            &stored_dep.selected,
        )
        .await?;
        let availability =
            availability_from_completion(&tx, tenant, project, &trusted_completion).await?;
        let current_selection =
            current_selection_for_policy(&tx, tenant, project, &stored_dep).await?;
        let mut trusted_req = req.clone();
        trusted_req.dependency = stored_dep;
        trusted_req.export_authorization = stored_export;
        trusted_req.completion = trusted_completion;
        trusted_req.availability = availability;
        trusted_req.current_selection = current_selection;
        let cred = core_adopt_delivery_credential(&trusted_req).map_err(map_delivery)?;
        persist_credential(&tx, tenant, project, &cred).await?;
        let receipt = record_receipt(
            &tx,
            tenant,
            project,
            &req.request_key,
            "adopt",
            &request_hash,
            &cred,
        )
        .await?;
        tx.commit().await?;
        Ok((cred, receipt))
    }
}

fn missing_trusted_completion() -> PgError {
    PgError::Protocol("trusted WS-018 completion proof missing".into())
}

async fn current_selection_for_policy(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    dep: &HardDeliveryDependency,
) -> PgResult<Option<DeliveryVersion>> {
    if dep.policy == awr_core::DeliveryVersionPolicy::FixedDelivery {
        return Ok(None);
    }
    let row = tx
        .query_opt(
            "SELECT state, selected_completion_id FROM awr_team.work_runtime
             WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3",
            &[&tenant, &project, &dep.provider.work_item_id],
        )
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let state: String = row.get(0);
    let selected: Option<String> = row.get(1);
    if state != "completed" {
        return Ok(None);
    }
    let Some(selected) = selected else {
        return Ok(None);
    };
    if selected == dep.selected.completion_receipt.to_string() {
        return Ok(Some(dep.selected.clone()));
    }
    let receipt_id: Id = selected
        .parse()
        .map_err(|_| PgError::Protocol("current completion id is not a ulid".into()))?;
    Ok(Some(DeliveryVersion {
        completion_receipt: receipt_id,
        ..dep.selected.clone()
    }))
}

async fn availability_from_completion(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    proof: &CompletionAcceptanceProof,
) -> PgResult<DeliveryAvailability> {
    let evidence_id = proof.evidence_id.to_string();
    let row = tx
        .query_opt(
            "SELECT e.output_digest, a.state
             FROM awr_team.evidence e
             LEFT JOIN awr_team.artifacts a
               ON a.tenant_id=e.tenant_id AND a.project_id=e.project_id AND a.id=e.artifact_id
             WHERE e.tenant_id=$1 AND e.project_id=$2 AND e.id=$3",
            &[&tenant, &project, &evidence_id],
        )
        .await?
        .ok_or_else(missing_trusted_completion)?;
    let output: Option<String> = row.get(0);
    let artifact_state: Option<String> = row.get(1);
    if output.as_deref() != Some(proof.artifact_sha256.as_str()) {
        return Err(PgError::Protocol(
            "trusted completion artifact binding mismatch".into(),
        ));
    }
    if artifact_state.as_deref() == Some("missing") {
        return Ok(DeliveryAvailability::Unavailable);
    }
    Ok(DeliveryAvailability::Available)
}

async fn load_trusted_completion_proof(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    provider_work_id: &str,
    selected: &DeliveryVersion,
) -> PgResult<CompletionAcceptanceProof> {
    let receipt_id = selected.completion_receipt.to_string();
    let receipt = tx
        .query_opt(
            "SELECT work_id, contract_hash, independence_kind, evidence_id,
                    approved_by_person_id, submitted_by_person_id,
                    (EXTRACT(EPOCH FROM accepted_at) * 1000)::bigint
             FROM awr_team.completion_receipts
             WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main'
               AND work_id=$3 AND id=$4",
            &[&tenant, &project, &provider_work_id, &receipt_id],
        )
        .await?
        .ok_or_else(missing_trusted_completion)?;
    let work_id: String = receipt.get(0);
    let contract_hash: String = receipt.get(1);
    let independence_kind: Option<String> = receipt.get(2);
    let evidence_id: Option<String> = receipt.get(3);
    let reviewer_person: Option<String> = receipt.get(4);
    let author_person: Option<String> = receipt.get(5);
    let verified_at_ms: i64 = receipt.get(6);

    if work_id != provider_work_id || contract_hash != selected.contract_sha256 {
        return Err(PgError::Protocol(
            "trusted completion contract/work binding mismatch".into(),
        ));
    }
    let evidence_id = evidence_id.ok_or_else(missing_trusted_completion)?;
    let independence_kind = independence_kind.ok_or_else(missing_trusted_completion)?;
    let author_person = author_person.ok_or_else(missing_trusted_completion)?;
    let reviewer_person = reviewer_person.ok_or_else(missing_trusted_completion)?;
    if author_person.is_empty() || reviewer_person.is_empty() {
        return Err(missing_trusted_completion());
    }

    let evidence = tx
        .query_opt(
            "SELECT work_id, contract_hash, output_digest, trust_basis, digest
             FROM awr_team.evidence
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &evidence_id],
        )
        .await?
        .ok_or_else(missing_trusted_completion)?;
    let ev_work: String = evidence.get(0);
    let ev_contract: String = evidence.get(1);
    let output_digest: Option<String> = evidence.get(2);
    let trust_basis: String = evidence.get(3);
    let bundle_hash: String = evidence.get(4);
    let artifact_sha256 = output_digest.ok_or_else(missing_trusted_completion)?;
    if ev_work != provider_work_id
        || ev_contract != selected.contract_sha256
        || artifact_sha256 != selected.artifact_sha256
    {
        return Err(PgError::Protocol(
            "trusted completion evidence binding mismatch".into(),
        ));
    }

    // Independent-review binding: a still-approved round must have an approve
    // decision by the receipt's reviewer, and that person must not be the author.
    if independence_kind == "team_independent" {
        let approved = tx
            .query_opt(
                "SELECT 1
                 FROM awr_team.review_rounds rr
                 JOIN awr_team.review_decisions rd
                   ON rd.tenant_id=rr.tenant_id AND rd.project_id=rr.project_id
                  AND rd.review_round_id=rr.id
                 WHERE rr.tenant_id=$1 AND rr.project_id=$2 AND rr.work_id=$3
                   AND rr.bundle_hash=$4 AND rr.contract_hash=$5
                   AND rr.state='approved'
                   AND rd.decision='approve'
                   AND rd.independence_kind='team_independent'
                   AND rd.reviewer_person_id=$6
                   AND rd.reviewer_person_id <> $7
                 LIMIT 1",
                &[
                    &tenant,
                    &project,
                    &provider_work_id,
                    &bundle_hash,
                    &selected.contract_sha256,
                    &reviewer_person,
                    &author_person,
                ],
            )
            .await?;
        if approved.is_none() {
            return Err(PgError::Protocol(
                "trusted completion independent-review binding missing".into(),
            ));
        }
    }

    let evidence_level = match trust_basis.as_str() {
        "trusted_executor" | "human_review" => EvidenceLevel::LocallyVerified,
        _ => EvidenceLevel::Unknown,
    };
    let evidence_ulid: Id = evidence_id
        .parse()
        .map_err(|_| PgError::Protocol("completion evidence id is not a ulid".into()))?;
    if verified_at_ms < 0 {
        return Err(PgError::Protocol(
            "trusted completion has invalid time".into(),
        ));
    }

    Ok(CompletionAcceptanceProof {
        completion_receipt_id: selected.completion_receipt,
        work_item_id: provider_work_id.to_string(),
        contract_sha256: selected.contract_sha256.clone(),
        artifact_sha256: selected.artifact_sha256.clone(),
        independence_kind: independence_kind.clone(),
        team_independent_acceptance: independence_kind == "team_independent",
        author_person_id: author_person,
        reviewer_person_id: reviewer_person,
        evidence_id: evidence_ulid,
        evidence_level,
        verified_at_ms,
    })
}

async fn load_dep(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    id: &str,
) -> PgResult<Option<HardDeliveryDependency>> {
    let row = tx
        .query_opt(
            "SELECT body_json FROM awr_team.hard_delivery_dependencies
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &id],
        )
        .await?;
    match row {
        None => Ok(None),
        Some(r) => {
            let v: Value = r.get(0);
            Ok(Some(serde_json::from_value(v).map_err(|e| {
                PgError::Protocol(format!("corrupt hard delivery dependency: {e}"))
            })?))
        }
    }
}

async fn persist_dep(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    dep: &HardDeliveryDependency,
) -> PgResult<()> {
    let body = serde_json::to_value(dep).map_err(|e| PgError::Protocol(e.to_string()))?;
    let provider_ws = dep.provider.workstream_id.to_string();
    let consumer_ws = dep.consumer.workstream_id.to_string();
    let policy = policy_str(dep.policy);
    let status = dep_status_str(dep.status);
    let receipt = dep.selected.completion_receipt.to_string();
    tx.execute(
        "INSERT INTO awr_team.hard_delivery_dependencies(
            tenant_id,project_id,id,provider_work_id,provider_workstream_id,
            consumer_work_id,consumer_workstream_id,policy,status,completion_receipt,
            contract_sha256,artifact_sha256,created_at_ms,revoked_at_ms,body_json)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)
         ON CONFLICT (tenant_id, project_id, id) DO UPDATE SET
            status=EXCLUDED.status,
            revoked_at_ms=EXCLUDED.revoked_at_ms,
            body_json=EXCLUDED.body_json",
        &[
            &tenant,
            &project,
            &dep.id,
            &dep.provider.work_item_id,
            &provider_ws,
            &dep.consumer.work_item_id,
            &consumer_ws,
            &policy,
            &status,
            &receipt,
            &dep.selected.contract_sha256,
            &dep.selected.artifact_sha256,
            &dep.created_at_ms,
            &dep.revoked_at_ms,
            &body,
        ],
    )
    .await?;
    Ok(())
}

async fn load_export(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    id: &str,
) -> PgResult<Option<ExportAuthorization>> {
    let row = tx
        .query_opt(
            "SELECT body_json FROM awr_team.export_authorizations
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &id],
        )
        .await?;
    match row {
        None => Ok(None),
        Some(r) => {
            let v: Value = r.get(0);
            Ok(Some(serde_json::from_value(v).map_err(|e| {
                PgError::Protocol(format!("corrupt export authorization: {e}"))
            })?))
        }
    }
}

async fn persist_export(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ExportAuthorization,
) -> PgResult<()> {
    let body = serde_json::to_value(auth).map_err(|e| PgError::Protocol(e.to_string()))?;
    let status = export_status_str(auth.status);
    let receipt = auth.delivery.completion_receipt.to_string();
    tx.execute(
        "INSERT INTO awr_team.export_authorizations(
            tenant_id,project_id,id,provider_work_id,status,completion_receipt,contract_sha256,
            artifact_sha256,export_scope_sha256,granted_by,created_at_ms,revoked_at_ms,body_json)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
         ON CONFLICT (tenant_id, project_id, id) DO UPDATE SET
            status=EXCLUDED.status,
            revoked_at_ms=EXCLUDED.revoked_at_ms,
            body_json=EXCLUDED.body_json",
        &[
            &tenant,
            &project,
            &auth.id,
            &auth.provider_work_item_id,
            &status,
            &receipt,
            &auth.delivery.contract_sha256,
            &auth.delivery.artifact_sha256,
            &auth.delivery.export_scope_sha256,
            &auth.granted_by,
            &auth.created_at_ms,
            &auth.revoked_at_ms,
            &body,
        ],
    )
    .await?;
    Ok(())
}

async fn load_credential(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    id: &str,
) -> PgResult<Option<AdoptionCredential>> {
    let row = tx
        .query_opt(
            "SELECT body_json FROM awr_team.adoption_credentials
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &id],
        )
        .await?;
    match row {
        None => Ok(None),
        Some(r) => {
            let v: Value = r.get(0);
            Ok(Some(serde_json::from_value(v).map_err(|e| {
                PgError::Protocol(format!("corrupt adoption credential: {e}"))
            })?))
        }
    }
}

async fn persist_credential(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    cred: &AdoptionCredential,
) -> PgResult<()> {
    let body = serde_json::to_value(cred).map_err(|e| PgError::Protocol(e.to_string()))?;
    let status = cred_status_str(cred.status);
    let receipt = cred.completion_receipt_id.to_string();
    tx.execute(
        "INSERT INTO awr_team.adoption_credentials(
            tenant_id,project_id,id,dependency_id,status,completion_receipt,
            export_authorization_id,adopted_at_ms,body_json)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
         ON CONFLICT (tenant_id, project_id, id) DO UPDATE SET
            status=EXCLUDED.status,
            body_json=EXCLUDED.body_json",
        &[
            &tenant,
            &project,
            &cred.id,
            &cred.dependency_id,
            &status,
            &receipt,
            &cred.export_authorization_id,
            &cred.adopted_at_ms,
            &body,
        ],
    )
    .await?;
    Ok(())
}

// Bind even caller-supplied snapshots and observation times: replacing an ignored
// proof field is still a different request, not an identical result recovery.
fn request_digest<T: Serialize>(
    tenant: &str,
    project: &str,
    op: &str,
    request: &T,
) -> PgResult<String> {
    awr_team::request_hash(&json!({
        "domain": "awr-team-legacy-delivery-request-v1",
        "tenant_id": tenant,
        "project_id": project,
        "op": op,
        "request": request,
    }))
    .map_err(|_| PgError::Protocol("invalid delivery request binding".into()))
}

async fn load_receipt<T: DeserializeOwned>(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    request_key: &str,
    op: &str,
    request_hash: &str,
) -> PgResult<Option<(T, DeliveryCredentialReceipt)>> {
    let Some(row) = tx
        .query_opt(
            "SELECT subject_id,op,event_id,request_hash,result_json
         FROM awr_team.delivery_credential_receipts
         WHERE tenant_id=$1 AND project_id=$2 AND request_key=$3",
            &[&tenant, &project, &request_key],
        )
        .await?
    else {
        return Ok(None);
    };
    let subject_id: String = row.get(0);
    let stored_op: String = row.get(1);
    let event_id: String = row.get(2);
    let stored_hash: Option<String> = row.get(3);
    let original: Option<Value> = row.get(4);
    if stored_op != op {
        return Err(PgError::IdempotencyConflict);
    }
    let (Some(stored_hash), Some(original)) = (stored_hash, original) else {
        return Err(PgError::Protocol(
            "legacy delivery receipt lacks its original request or result; inspect current dependency, export and credential records before a new validated operation".into(),
        ));
    };
    if stored_hash != request_hash {
        return Err(PgError::IdempotencyConflict);
    }
    if original.get("id").and_then(Value::as_str) != Some(subject_id.as_str()) {
        return Err(PgError::Protocol(
            "invalid original delivery result binding".into(),
        ));
    }
    let result = serde_json::from_value(original)
        .map_err(|_| PgError::Protocol("invalid original delivery result".into()))?;
    // This is the original outcome, not live authority. Admission and current
    // adoption validity must still read current dependency/export/proof records.
    Ok(Some((
        result,
        DeliveryCredentialReceipt {
            request_key: request_key.into(),
            subject_id,
            op: stored_op,
            event_id: event_id
                .parse()
                .map_err(|_| PgError::Protocol("receipt event id is not a ulid".into()))?,
            replayed: true,
        },
    )))
}

async fn record_receipt<T: Serialize>(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    request_key: &str,
    op: &str,
    request_hash: &str,
    subject: &T,
) -> PgResult<DeliveryCredentialReceipt> {
    let original = serde_json::to_value(subject)
        .map_err(|_| PgError::Protocol("invalid original delivery result".into()))?;
    let subject_id = original
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| PgError::Protocol("invalid original delivery result binding".into()))?;
    let event_id = new_id();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    tx.execute(
        "INSERT INTO awr_team.delivery_credential_receipts(
            tenant_id,project_id,request_key,subject_id,op,event_id,replayed,created_at_ms,request_hash,result_json)
         VALUES ($1,$2,$3,$4,$5,$6,false,$7,$8,$9)",
        &[&tenant,&project,&request_key,&subject_id,&op,&event_id,&now,&request_hash,&original],
    ).await?;
    Ok(DeliveryCredentialReceipt {
        request_key: request_key.into(),
        subject_id: subject_id.into(),
        op: op.into(),
        event_id: event_id
            .parse()
            .map_err(|_| PgError::Protocol("event id is not a ulid".into()))?,
        replayed: false,
    })
}
