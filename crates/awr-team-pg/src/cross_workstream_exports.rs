//! Approved-artifact disclosure on the ordinary authenticated command plane.
//! Publication grants bounded content access, never adoption or execution rights.
use crate::workstream_auth::ReaderAuthority;
use crate::workstream_command::{WorkstreamCommand, session};
use crate::workstream_read::WorkstreamQuery;
use crate::{PgError, PgResult};
use awr_team::{
    CrossWorkstreamDependencyPolicy, CrossWorkstreamReviewAssurance, DependencyAcceptanceMode,
    WorkContract,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_postgres::Transaction;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Publish {
    session_id: String,
    expected_session_version: String,
    consumer_work_id: String,
    expected_consumer_contract_hash: String,
    expected_consumer_ownership_version: String,
    receipt_id: String,
    expected_artifact_sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Revoke {
    session_id: String,
    expected_session_version: String,
    export_id: String,
    expected_export_version: String,
}

pub(crate) enum Action {
    Publish(Publish),
    Revoke(Revoke),
}

impl Action {
    pub(crate) fn parse(op: &str, args: Value) -> PgResult<Self> {
        let action = match op {
            "delivery.export.publish" => {
                Self::Publish(serde_json::from_value(args).map_err(|_| invalid())?)
            }
            "delivery.export.revoke" => {
                Self::Revoke(serde_json::from_value(args).map_err(|_| invalid())?)
            }
            _ => return Err(invalid()),
        };
        let (sid, version) = match &action {
            Self::Publish(a) => {
                if !id(&a.consumer_work_id)
                    || !id(&a.receipt_id)
                    || !digest(&a.expected_consumer_contract_hash)
                    || !digest(&a.expected_artifact_sha256)
                    || positive(&a.expected_consumer_ownership_version).is_err()
                {
                    return Err(invalid());
                }
                (&a.session_id, &a.expected_session_version)
            }
            Self::Revoke(a) => {
                if !id(&a.export_id) || positive(&a.expected_export_version).is_err() {
                    return Err(invalid());
                }
                (&a.session_id, &a.expected_session_version)
            }
        };
        if !id(sid) || positive(version).is_err() {
            return Err(invalid());
        }
        Ok(action)
    }
}

fn invalid() -> PgError {
    PgError::invalid_command_fields()
}
fn id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control)
}
fn digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn positive(s: &str) -> PgResult<i64> {
    s.parse::<i64>()
        .ok()
        .filter(|n| *n > 0 && n.to_string() == s)
        .ok_or_else(invalid)
}
fn hash(v: &Value) -> PgResult<String> {
    awr_team::request_hash(v).map_err(|_| PgError::SourceDivergence)
}

struct SourceWork {
    contract: WorkContract,
    hash: String,
    stream: String,
    ownership: i64,
}

async fn source_work(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    snapshot: &str,
    work: &str,
) -> PgResult<SourceWork> {
    // Source-owned selectors only. Reading this record internally grants no upstream access.
    let r = tx
        .query_opt(
            "SELECT c.contract_json,c.contract_hash,o.workstream_id,o.ownership_version
        FROM awr_team.work_contracts c JOIN awr_team.workstream_snapshot_ownership o
          ON o.tenant_id=c.tenant_id AND o.project_id=c.project_id AND o.snapshot_id=c.snapshot_id
          AND o.scope_id=c.scope_id AND o.work_id=c.work_id
        WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.snapshot_id=$3 AND c.scope_id='main'
          AND c.work_id=$4 AND c.definition_state='enabled'",
            &[&tenant, &project, &snapshot, &work],
        )
        .await?
        .ok_or(PgError::MissingDependency)?;
    let contract: WorkContract =
        serde_json::from_value(r.get(0)).map_err(|_| PgError::SourceDivergence)?;
    let stored: String = r.get(1);
    if contract.work_id.as_str() != work
        || contract.hash().map_err(|_| PgError::SourceDivergence)? != stored
    {
        return Err(PgError::SourceDivergence);
    }
    Ok(SourceWork {
        contract,
        hash: stored,
        stream: r.get(2),
        ownership: r.get(3),
    })
}

fn edge(
    provider: &str,
    upstream: &SourceWork,
    downstream: &SourceWork,
) -> PgResult<CrossWorkstreamDependencyPolicy> {
    if upstream.stream == downstream.stream
        || !downstream
            .contract
            .required_dependencies
            .iter()
            .any(|w| w == provider)
    {
        return Err(PgError::MissingDependency);
    }
    match downstream.contract.dependency_acceptance.get(provider) {
        Some(DependencyAcceptanceMode::CrossWorkstream(policy)) => Ok(*policy),
        _ => Err(PgError::MissingDependency),
    }
}

/// Publication requires a current selection; fixed reads bind accepted history.
#[derive(Clone, Copy)]
enum ProofVersion {
    CurrentSelected,
    FixedAccepted,
}

async fn proof(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    snapshot: &str,
    work: &str,
    receipt: &str,
    assurance: CrossWorkstreamReviewAssurance,
    version: ProofVersion,
) -> PgResult<(Value, Value)> {
    let historical = matches!(version, ProofVersion::FixedAccepted);
    let r = tx.query_opt("SELECT r.id,r.independence_kind,r.policy,r.approved_by_json,
            r.contract_hash,r.evidence_id,c.snapshot_id,r.execution_id,
            e.execution_id,r.evidence_bundle_hash,e.digest,r.result_digest,
            to_jsonb(r),to_jsonb(e),c.contract_json
         FROM awr_team.completion_receipts r LEFT JOIN awr_team.work_runtime w
           ON w.tenant_id=r.tenant_id AND w.project_id=r.project_id AND w.scope_id=r.scope_id
           AND w.work_id=r.work_id
         JOIN awr_team.projects p ON p.tenant_id=r.tenant_id AND p.id=r.project_id
         JOIN awr_team.work_contracts c ON c.tenant_id=r.tenant_id AND c.project_id=r.project_id
           AND c.snapshot_id=$5 AND c.scope_id=r.scope_id AND c.work_id=r.work_id
           AND c.contract_hash=r.contract_hash AND c.definition_state='enabled'
         JOIN awr_team.evidence e ON e.tenant_id=r.tenant_id AND e.project_id=r.project_id AND e.id=r.evidence_id
           AND e.work_id=r.work_id AND e.contract_hash=r.contract_hash
         WHERE r.tenant_id=$1 AND r.project_id=$2 AND r.scope_id='main' AND r.work_id=$3 AND r.id=$4
           AND ($6 OR (p.active_snapshot_id=$5 AND w.state='completed'
                AND w.selected_completion_id=r.id AND NOT w.recovery_blocked))",
        &[&tenant,&project,&work,&receipt,&snapshot,&historical]).await?.ok_or(PgError::EvidenceInvalid)?;
    let basis: Value = r.get(3);
    let evidence: Value = r.get(13);
    let contract_hash: String = r.get(4);
    let bundle: String = r.get(9);
    if r.get::<_, String>(10) != bundle
        || r.get::<_, String>(11) != bundle
        || crate::review::evidence_digest(
            work,
            &contract_hash,
            evidence["input_digest"].as_str(),
            evidence["output_digest"].as_str(),
            evidence["execution_result_digest"].as_str(),
            &evidence["payload_json"],
        )? != bundle
    {
        return Err(PgError::EvidenceInvalid);
    }
    let execution: Option<String> = r.get(7);
    let (Some(execution), Some(input), Some(result)) = (
        execution.as_deref(),
        evidence["input_digest"].as_str(),
        evidence["execution_result_digest"].as_str(),
    ) else {
        return Err(PgError::EvidenceInvalid);
    };
    if r.get::<_, Option<String>>(8).as_deref() != Some(execution)
        || evidence["payload_json"]["passed"] == false
        || basis["execution_success"] != true
    {
        return Err(PgError::EvidenceInvalid);
    }
    let successful: bool = tx
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM awr_team.executions
         WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND work_id=$4 AND scope_id='main'
           AND contract_hash=$5 AND input_digest=$6 AND result_digest=$7 AND state='succeeded')",
            &[
                &tenant,
                &project,
                &execution,
                &work,
                &contract_hash,
                &input,
                &result,
            ],
        )
        .await?
        .get(0);
    if !successful {
        return Err(PgError::EvidenceInvalid);
    }
    let contract: WorkContract =
        serde_json::from_value(r.get(14)).map_err(|_| PgError::EvidenceInvalid)?;
    if contract.work_id.as_str() != work
        || contract.hash().map_err(|_| PgError::EvidenceInvalid)? != contract_hash
    {
        return Err(PgError::EvidenceInvalid);
    }
    if contract
        .dependency_acceptance
        .values()
        .any(|m| matches!(m, DependencyAcceptanceMode::CrossWorkstream(_)))
    {
        let links: std::collections::BTreeMap<String, String> = tx
            .query("SELECT predecessor_work_id,predecessor_completion_id FROM awr_team.completion_dependencies
                WHERE tenant_id=$1 AND project_id=$2 AND completion_id=$3", &[&tenant,&project,&receipt])
            .await?.into_iter().map(|r| (r.get(0),r.get(1))).collect();
        if links.len() != contract.required_dependencies.len() {
            return Err(PgError::EvidenceInvalid);
        }
        let inputs = contract
            .required_dependencies
            .iter()
            .map(|w| {
                links
                    .get(w)
                    .cloned()
                    .map(|receipt| (w.clone(), receipt))
                    .ok_or(PgError::EvidenceInvalid)
            })
            .collect::<PgResult<Vec<_>>>()?;
        crate::cross_workstream_adoption::require_execution_inputs(
            tx,
            tenant,
            project,
            work,
            &contract,
            Some(execution),
            &inputs,
        )
        .await?;
    }
    let (Some(round), Some(decision)) = (
        basis["review_round_id"].as_str(),
        basis["review_decision_id"].as_str(),
    ) else {
        return Err(PgError::ReviewRequired);
    };
    let approval = tx.query_opt("SELECT to_jsonb(r),to_jsonb(d) FROM awr_team.review_rounds r JOIN awr_team.review_decisions d
        ON d.tenant_id=r.tenant_id AND d.project_id=r.project_id AND d.review_round_id=r.id
        WHERE r.tenant_id=$1 AND r.project_id=$2 AND r.id=$3 AND d.id=$4 AND r.work_id=$5
          AND d.work_id=r.work_id AND r.contract_hash=$6 AND r.bundle_hash=$7 AND d.bundle_hash=r.bundle_hash
          AND r.state='approved' AND d.decision='approve'", &[&tenant,&project,&round,&decision,&work,&contract_hash,&bundle]).await?.ok_or(PgError::ReviewRequired)?;
    let original_round: Value = approval.get(0);
    let original_decision: Value = approval.get(1);
    if original_round["evidence_id"] != evidence["id"]
        || original_round["artifact_digest"] != evidence["output_digest"]
        || original_round["execution_id"] != evidence["execution_id"]
        || original_round["execution_result_digest"] != evidence["execution_result_digest"]
        || original_decision["reviewer_actor_id"] != basis["approved_by"]
    {
        return Err(PgError::ReviewRequired);
    }
    match assurance {
        CrossWorkstreamReviewAssurance::SimulatedMemberIndependent => {
            if !crate::review::dependency_receipt_accepted(
                Some(DependencyAcceptanceMode::SimulatedMemberIndependent),
                r.get::<_, Option<String>>(1).as_deref(),
                &r.get::<_, String>(2),
                &basis,
            ) || !crate::review::simulated_dependency_verified(
                tx, tenant, project, work, &r, &basis,
            )
            .await?
            {
                return Err(PgError::ReviewRequired);
            }
        }
        CrossWorkstreamReviewAssurance::TeamIndependent => {
            if r.get::<_, Option<String>>(1).as_deref() != Some("team_independent")
                || basis["human_approval"] != true
                || basis["team_independent_acceptance"] != true
                || basis["execution_basis"] != "trusted_execution_receipt"
                || evidence["trust_basis"] != "trusted_executor"
                || original_decision["independence_kind"] != "team_independent"
                || original_round["author_person_id"].as_str().is_none()
                || original_decision["reviewer_person_id"].as_str().is_none()
                || original_decision["reviewer_person_id"] == original_round["author_person_id"]
                || original_decision["reviewer_person_id"] != basis["approved_by_person_id"]
            {
                return Err(PgError::ReviewRequired);
            }
            let actor: String = tx.query_one(
                "SELECT executor_actor_id FROM awr_team.executions WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant,&project,&execution],
            ).await?.get(0);
            if evidence["created_by"] != actor {
                return Err(PgError::EvidenceInvalid);
            }
        }
    }
    let archive = json!({"receipt":r.get::<_,Value>(12),"evidence":evidence,"round":original_round,"decision":original_decision});
    let summary = json!({"approval_basis":basis["approval_basis"],"execution_basis":basis["execution_basis"],
        "human_approval":basis["human_approval"],"team_independent_acceptance":basis["team_independent_acceptance"],
        "review_basis_sha256":hash(&archive)?});
    Ok((archive, summary))
}

async fn artifact(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    artifact: &str,
    expected: &str,
) -> PgResult<(Vec<u8>, Option<String>)> {
    let r = tx
        .query_opt(
            "SELECT content,sha256,byte_length,media_type,state FROM awr_team.artifacts
        WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &artifact],
        )
        .await?
        .ok_or(PgError::EvidenceInvalid)?;
    let bytes: Vec<u8> = r
        .get::<_, Option<Vec<u8>>>(0)
        .ok_or(PgError::EvidenceInvalid)?;
    if r.get::<_, String>(4) != "finalized"
        || r.get::<_, String>(1) != expected
        || crate::source::sha256_hex(&bytes) != expected
        || r.get::<_, i64>(2) != bytes.len() as i64
    {
        return Err(PgError::EvidenceInvalid);
    }
    Ok((bytes, r.get(3)))
}

pub(crate) async fn apply(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    ownership: i64,
    action: Action,
) -> PgResult<Value> {
    let (sid, version) = match &action {
        Action::Publish(a) => (&a.session_id, &a.expected_session_version),
        Action::Revoke(a) => (&a.session_id, &a.expected_session_version),
    };
    session(tx, tenant, project, auth, command, sid, version, ownership).await?;
    match action {
        Action::Publish(a) => {
            let provider =
                source_work(tx, tenant, project, &auth.snapshot, &command.work_id).await?;
            let consumer =
                source_work(tx, tenant, project, &auth.snapshot, &a.consumer_work_id).await?;
            if consumer.hash != a.expected_consumer_contract_hash
                || consumer.ownership != positive(&a.expected_consumer_ownership_version)?
            {
                return Err(PgError::PreconditionsChanged);
            }
            let policy = edge(&command.work_id, &provider, &consumer)?;
            let (archive, summary) = Box::pin(proof(
                tx,
                tenant,
                project,
                &auth.snapshot,
                &command.work_id,
                &a.receipt_id,
                policy.review_assurance,
                ProofVersion::CurrentSelected,
            ))
            .await?;
            let artifact_id = archive["evidence"]["artifact_id"]
                .as_str()
                .ok_or(PgError::EvidenceInvalid)?;
            let (bytes, media) = artifact(
                tx,
                tenant,
                project,
                artifact_id,
                &a.expected_artifact_sha256,
            )
            .await?;
            if archive["evidence"]["output_digest"] != a.expected_artifact_sha256 {
                return Err(PgError::EvidenceInvalid);
            }
            let fixed = policy.version_policy == awr_core::DeliveryVersionPolicy::FixedDelivery;
            let mut manifest = json!({"codec":if fixed {"awr-approved-artifact-export-v2"} else {"awr-approved-artifact-export-v1"},"provider_work_id":command.work_id,
                "provider_workstream_id":provider.stream,"provider_ownership_version":provider.ownership.to_string(),"provider_contract_hash":provider.hash,
                "consumer_work_id":a.consumer_work_id,"consumer_workstream_id":consumer.stream,
                "consumer_ownership_version":consumer.ownership.to_string(),"consumer_contract_hash":consumer.hash,
                "receipt_id":a.receipt_id,"artifact_id":artifact_id,"artifact_sha256":a.expected_artifact_sha256,
                "byte_length":bytes.len(),"media_type":media,"policy":policy,"review":summary,"repository_source_sha":Value::Null});
            if fixed {
                // Captured only while the provider's exact current completion is
                // verified. Never guess historical source for legacy exports.
                manifest["provider_source_snapshot_id"] = json!(auth.snapshot);
            }
            let disclosure = hash(&manifest)?;
            if let Some(r) = tx
                .query_opt(
                    "SELECT id,version FROM awr_team.workstream_artifact_exports
                WHERE tenant_id=$1 AND project_id=$2 AND disclosure_sha256=$3 AND status='active'",
                    &[&tenant, &project, &disclosure],
                )
                .await?
            {
                let adopted = crate::cross_workstream_adoption::is_selected_export(
                    tx,
                    tenant,
                    project,
                    &a.consumer_work_id,
                    &r.get::<_, String>(0),
                    r.get(1),
                    &disclosure,
                    &manifest,
                )
                .await?;
                return Ok(
                    json!({"export_id":r.get::<_,String>(0),"export_version":r.get::<_,i64>(1).to_string(),"disclosure_sha256":disclosure,"status":"active","already_published":true,"adopted":adopted}),
                );
            }
            let export = crate::tx::new_id();
            tx.execute("INSERT INTO awr_team.workstream_artifact_exports(
                tenant_id,project_id,id,provider_work_id,consumer_work_id,receipt_id,artifact_id,disclosure_sha256,manifest_json,proof_json,published_by_actor_id,published_by_client_id)
                VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)", &[&tenant,&project,&export,&command.work_id,&a.consumer_work_id,&a.receipt_id,&artifact_id,&disclosure,&manifest,&archive,&auth.actor_id,&auth.client_id]).await?;
            Ok(
                json!({"export_id":export,"export_version":"1","disclosure_sha256":disclosure,"status":"active","already_published":false,"adopted":false}),
            )
        }
        Action::Revoke(a) => {
            let r = tx.query_opt("UPDATE awr_team.workstream_artifact_exports SET status='revoked',version=version+1,
                revoked_by_actor_id=$6,revoked_by_client_id=$7,revoked_at=clock_timestamp()
                WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND provider_work_id=$4 AND version=$5 AND status='active' RETURNING version,disclosure_sha256",
                &[&tenant,&project,&a.export_id,&command.work_id,&positive(&a.expected_export_version)?,&auth.actor_id,&auth.client_id]).await?.ok_or(PgError::PreconditionsChanged)?;
            Ok(
                json!({"export_id":a.export_id,"export_version":r.get::<_,i64>(0).to_string(),"disclosure_sha256":r.get::<_,String>(1),"status":"revoked","adopted":false}),
            )
        }
    }
}

async fn live(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    snapshot: &str,
    consumer_work: &str,
    manifest: &Value,
    archive: &Value,
    disclosure: &str,
) -> PgResult<Vec<u8>> {
    if hash(manifest)? != disclosure || manifest["consumer_work_id"] != consumer_work {
        return Err(PgError::Forbidden);
    }
    let provider_work = manifest["provider_work_id"]
        .as_str()
        .ok_or(PgError::EvidenceInvalid)?;
    let provider = source_work(tx, tenant, project, snapshot, provider_work).await?;
    let consumer = source_work(tx, tenant, project, snapshot, consumer_work).await?;
    let policy = edge(provider_work, &provider, &consumer)?;
    let archived = if policy.version_policy == awr_core::DeliveryVersionPolicy::FixedDelivery {
        if manifest["codec"] != "awr-approved-artifact-export-v2" {
            return Err(PgError::EvidenceInvalid);
        }
        let original = manifest["provider_source_snapshot_id"]
            .as_str()
            .filter(|s| id(s))
            .ok_or(PgError::EvidenceInvalid)?;
        Some(source_work(tx, tenant, project, original, provider_work).await?)
    } else {
        if manifest["codec"] != "awr-approved-artifact-export-v1" {
            return Err(PgError::EvidenceInvalid);
        }
        None
    };
    let proof_provider = archived.as_ref().unwrap_or(&provider);
    if manifest["provider_contract_hash"] != proof_provider.hash
        || manifest["consumer_contract_hash"] != consumer.hash
        || manifest["provider_workstream_id"] != proof_provider.stream
        || manifest["consumer_workstream_id"] != consumer.stream
        || manifest["provider_ownership_version"] != proof_provider.ownership.to_string()
        || manifest["consumer_ownership_version"] != consumer.ownership.to_string()
        || manifest["policy"] != json!(policy)
    {
        return Err(PgError::PreconditionsChanged);
    }
    let (actual, summary) = Box::pin(proof(
        tx,
        tenant,
        project,
        manifest["provider_source_snapshot_id"]
            .as_str()
            .filter(|_| archived.is_some())
            .unwrap_or(snapshot),
        provider_work,
        manifest["receipt_id"]
            .as_str()
            .ok_or(PgError::EvidenceInvalid)?,
        policy.review_assurance,
        if archived.is_some() {
            ProofVersion::FixedAccepted
        } else {
            ProofVersion::CurrentSelected
        },
    ))
    .await?;
    if &actual != archive
        || manifest["review"] != summary
        || manifest["artifact_id"] != actual["evidence"]["artifact_id"]
    {
        return Err(PgError::ReviewRequired);
    }
    let (bytes, media) = artifact(
        tx,
        tenant,
        project,
        manifest["artifact_id"]
            .as_str()
            .ok_or(PgError::EvidenceInvalid)?,
        manifest["artifact_sha256"]
            .as_str()
            .ok_or(PgError::EvidenceInvalid)?,
    )
    .await?;
    if manifest["byte_length"] != json!(bytes.len()) || manifest["media_type"] != json!(media) {
        return Err(PgError::EvidenceInvalid);
    }
    Ok(bytes)
}

pub(crate) async fn validated_export(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    snapshot: &str,
    consumer: &str,
    export: &str,
) -> PgResult<(Value, i64, String)> {
    let r = tx.query_opt("SELECT manifest_json,proof_json,disclosure_sha256,version FROM awr_team.workstream_artifact_exports
        WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND consumer_work_id=$4 AND status='active'",
        &[&tenant,&project,&export,&consumer]).await?.ok_or(PgError::Forbidden)?;
    let manifest: Value = r.get(0);
    live(
        tx,
        tenant,
        project,
        snapshot,
        consumer,
        &manifest,
        &r.get(1),
        &r.get::<_, String>(2),
    )
    .await?;
    Ok((manifest, r.get(3), r.get(2)))
}

pub(crate) async fn content(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    work: &str,
    q: &WorkstreamQuery,
) -> PgResult<Value> {
    let export = q.export_id.as_deref().ok_or(PgError::Forbidden)?;
    let r = tx.query_opt("SELECT manifest_json,proof_json,disclosure_sha256,version FROM awr_team.workstream_artifact_exports
        WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND consumer_work_id=$4 AND status='active'", &[&tenant,&project,&export,&work]).await?.ok_or(PgError::Forbidden)?;
    let manifest: Value = r.get(0);
    let bytes = live(
        tx,
        tenant,
        project,
        &auth.snapshot,
        work,
        &manifest,
        &r.get(1),
        &r.get::<_, String>(2),
    )
    .await?;
    if q.expected_sha256
        .as_deref()
        .is_some_and(|sha| manifest["artifact_sha256"] != sha)
    {
        return Err(PgError::SnapshotDrift(export.into()));
    }
    if bytes.len() > q.max_context_bytes.unwrap_or(65536) {
        return Err(PgError::ContextIncomplete);
    }
    let text = String::from_utf8(bytes.clone()).ok();
    let adopted = crate::cross_workstream_adoption::is_selected_export(
        tx,
        tenant,
        project,
        work,
        export,
        r.get(3),
        &r.get::<_, String>(2),
        &manifest,
    )
    .await?;
    Ok(
        json!({"kind":"exported_artifact","export_id":export,"export_version":r.get::<_,i64>(3).to_string(),
        "work_id":work,"sha256":manifest["artifact_sha256"],"byte_length":bytes.len(),"media_type":manifest["media_type"],
        "disclosure_sha256":r.get::<_,String>(2),"review":manifest["review"],"text":text,
        "content_base64":if text.is_none() { json!(crate::workstream_read::base64_encode(&bytes)) } else { Value::Null },
        "adopted":adopted,"execution_authorized":false,"upstream_source_access":false}),
    )
}

#[derive(serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    binding: String,
    last_id: String,
}

pub(crate) async fn list(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    work: &str,
    q: &WorkstreamQuery,
) -> PgResult<Value> {
    let consumer = source_work(tx, tenant, project, &auth.snapshot, work).await?;
    let binding = hash(
        &json!({"op":"delivery.exports","actor":auth.actor_id,"client":auth.client_id,"authority":auth.binding,"epoch":auth.epoch,
        "membership":auth.membership_version,"source":auth.snapshot,"work":work,"contract":consumer.hash,"stream":consumer.stream,"ownership":consumer.ownership}),
    )?;
    let after = if let Some(raw) = &q.cursor {
        let cursor: Cursor = serde_json::from_str(raw).map_err(|_| PgError::CursorExpired)?;
        if cursor.binding != binding || !id(&cursor.last_id) {
            return Err(PgError::CursorExpired);
        }
        cursor.last_id
    } else {
        String::new()
    };
    let limit = i64::from(q.limit.unwrap_or(20));
    let rows = tx.query("SELECT id,manifest_json,proof_json,disclosure_sha256,version,status,provider_work_id
        FROM awr_team.workstream_artifact_exports WHERE tenant_id=$1 AND project_id=$2 AND consumer_work_id=$3 AND id>$4 ORDER BY id LIMIT $5",
        &[&tenant,&project,&work,&after,&(limit+1)]).await?;
    let mut items = vec![];
    for r in rows.iter().take(limit as usize) {
        let provider: String = r.get(6);
        if !matches!(
            consumer.contract.dependency_acceptance.get(&provider),
            Some(DependencyAcceptanceMode::CrossWorkstream(_))
        ) {
            continue;
        }
        let export: String = r.get(0);
        let manifest: Value = r.get(1);
        let status: String = r.get(5);
        let result = if status == "active" {
            live(
                tx,
                tenant,
                project,
                &auth.snapshot,
                work,
                &manifest,
                &r.get(2),
                &r.get::<_, String>(3),
            )
            .await
            .map(|_| ())
        } else {
            Err(PgError::PreconditionsChanged)
        };
        let available = match result {
            Ok(()) => true,
            Err(
                PgError::EvidenceInvalid
                | PgError::ReviewRequired
                | PgError::PreconditionsChanged
                | PgError::MissingDependency
                | PgError::Forbidden,
            ) => false,
            Err(error) => return Err(error),
        };
        let adopted = available
            && crate::cross_workstream_adoption::is_selected_export(
                tx,
                tenant,
                project,
                work,
                &export,
                r.get(4),
                &r.get::<_, String>(3),
                &manifest,
            )
            .await?;
        let mut item = json!({"export_id":export,"export_version":r.get::<_,i64>(4).to_string(),"provider_work_id":provider,
            "available":available,"status":if status == "revoked" {"revoked"} else if available {"active"} else {"requires_republication"},"adopted":adopted});
        if available {
            item["artifact"] = json!({"sha256":manifest["artifact_sha256"],"byte_length":manifest["byte_length"],"media_type":manifest["media_type"]});
            item["disclosure_sha256"] = json!(r.get::<_, String>(3));
            item["receipt_id"] = manifest["receipt_id"].clone();
            item["review"] = manifest["review"].clone();
            item["policy"] = manifest["policy"].clone();
        }
        items.push(item);
    }
    let next = if rows.len() > limit as usize {
        json!(
            serde_json::to_string(&Cursor {
                binding,
                last_id: rows[limit as usize - 1].get(0)
            })
            .map_err(|_| PgError::CursorExpired)?
        )
    } else {
        Value::Null
    };
    Ok(
        json!({"items":items,"next_cursor":next,"adoption_available":true,
            "adoption_version_policies":["current_contract","fixed_delivery"],"upstream_source_access":false}),
    )
}
