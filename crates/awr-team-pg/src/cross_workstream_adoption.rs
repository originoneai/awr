//! Consumer-owned, version-bound adoption on the authenticated command plane.
use crate::workstream_auth::ReaderAuthority;
use crate::workstream_command::{WorkstreamCommand, session, task_intake};
use crate::{PgError, PgResult};
use awr_team::{CrossWorkstreamDependencyPolicy, DependencyAcceptanceMode, WorkContract};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_postgres::Transaction;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Adopt {
    session_id: String,
    expected_session_version: String,
    expected_responsibility_version: String,
    export_id: String,
    expected_export_version: String,
    expected_disclosure_sha256: String,
    expected_adoption_version: String,
}

fn decimal(value: &str) -> PgResult<i64> {
    value
        .parse::<i64>()
        .ok()
        .filter(|n| *n >= 0 && n.to_string() == value)
        .ok_or_else(PgError::invalid_command_fields)
}

pub(crate) fn parse(args: Value) -> PgResult<Adopt> {
    let a: Adopt = serde_json::from_value(args).map_err(|_| PgError::invalid_command_fields())?;
    if [&a.session_id, &a.export_id]
        .iter()
        .any(|s| s.is_empty() || s.len() > 128 || s.chars().any(char::is_control))
        || decimal(&a.expected_session_version)? == 0
        || decimal(&a.expected_export_version)? == 0
        || a.expected_disclosure_sha256.len() != 64
        || !a
            .expected_disclosure_sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(PgError::invalid_command_fields());
    }
    decimal(&a.expected_responsibility_version)?;
    decimal(&a.expected_adoption_version)?;
    Ok(a)
}

pub(crate) async fn apply(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    ownership: i64,
    a: Adopt,
) -> PgResult<Value> {
    session(
        tx,
        tenant,
        project,
        auth,
        command,
        &a.session_id,
        &a.expected_session_version,
        ownership,
    )
    .await?;
    task_intake::require_dependency_adopter(
        tx,
        tenant,
        project,
        auth,
        command,
        decimal(&a.expected_responsibility_version)? as u64,
    )
    .await?;
    let (manifest, export_version, disclosure) = crate::cross_workstream_exports::validated_export(
        tx,
        tenant,
        project,
        &auth.snapshot,
        &command.work_id,
        &a.export_id,
    )
    .await?;
    if export_version != decimal(&a.expected_export_version)?
        || disclosure != a.expected_disclosure_sha256
    {
        return Err(PgError::PreconditionsChanged);
    }
    let provider = manifest["provider_work_id"]
        .as_str()
        .ok_or(PgError::EvidenceInvalid)?;
    let current = selection(tx, tenant, project, &command.work_id, provider).await?;
    let version = current.as_ref().map(|r| r.get::<_, i64>(1)).unwrap_or(0);
    if version != decimal(&a.expected_adoption_version)? || version == i64::MAX {
        return Err(PgError::PreconditionsChanged);
    }
    if let Some(r) = &current {
        if r.get::<_, String>(2) == a.export_id
            && r.get::<_, i64>(3) == export_version
            && r.get::<_, String>(4) == disclosure
        {
            return Ok(
                json!({"adoption_id":r.get::<_,String>(0),"adoption_version":version.to_string(),
                "export_id":a.export_id,"export_version":export_version.to_string(),"provider_work_id":provider,
                "receipt_id":manifest["receipt_id"],"adopted":true,"already_adopted":true,"execution_authorized":false}),
            );
        }
    }
    crate::source::writeback::admission::require_dependency_clear(
        tx,
        tenant,
        project,
        &command.work_id,
    )
    .await?;
    tx.execute("UPDATE awr_team.workstream_artifact_adoptions SET selected=false
        WHERE tenant_id=$1 AND project_id=$2 AND consumer_work_id=$3 AND provider_work_id=$4 AND selected",
        &[&tenant,&project,&command.work_id,&provider]).await?;
    let id = crate::tx::new_id();
    tx.execute("INSERT INTO awr_team.workstream_artifact_adoptions(tenant_id,project_id,id,consumer_work_id,provider_work_id,
        export_id,export_version,disclosure_sha256,consumer_contract_hash,consumer_ownership_version,version,adopted_by_actor_id,adopted_by_client_id)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
        &[&tenant,&project,&id,&command.work_id,&provider,&a.export_id,&export_version,&disclosure,&command.expected_contract_hash,
            &ownership,&(version+1),&auth.actor_id,&auth.client_id]).await?;
    Ok(
        json!({"adoption_id":id,"adoption_version":(version+1).to_string(),"export_id":a.export_id,
        "export_version":export_version.to_string(),"provider_work_id":provider,"receipt_id":manifest["receipt_id"],
        "adopted":true,"already_adopted":false,"execution_authorized":false}),
    )
}

async fn selection(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    consumer: &str,
    provider: &str,
) -> PgResult<Option<tokio_postgres::Row>> {
    Ok(tx.query_opt("SELECT id,version,export_id,export_version,disclosure_sha256,consumer_contract_hash,consumer_ownership_version
        FROM awr_team.workstream_artifact_adoptions WHERE tenant_id=$1 AND project_id=$2 AND consumer_work_id=$3 AND provider_work_id=$4 AND selected",
        &[&tenant,&project,&consumer,&provider]).await?)
}

/// Display only, after the caller revalidates this exact export. Admission uses
/// `adopted_receipt`, which also checks the original proof and artifact bytes.
pub(crate) async fn is_selected_export(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    consumer: &str,
    export: &str,
    export_version: i64,
    disclosure: &str,
    manifest: &Value,
) -> PgResult<bool> {
    let provider = manifest["provider_work_id"]
        .as_str()
        .ok_or(PgError::EvidenceInvalid)?;
    let Some(r) = selection(tx, tenant, project, consumer, provider).await? else {
        return Ok(false);
    };
    Ok(r.get::<_, String>(2) == export
        && r.get::<_, i64>(3) == export_version
        && r.get::<_, String>(4) == disclosure
        && manifest["consumer_contract_hash"] == r.get::<_, String>(5)
        && manifest["consumer_ownership_version"] == r.get::<_, i64>(6).to_string())
}

fn ordinary_invalid(error: &PgError) -> bool {
    matches!(
        error,
        PgError::EvidenceInvalid
            | PgError::ReviewRequired
            | PgError::PreconditionsChanged
            | PgError::MissingDependency
            | PgError::Forbidden
    )
}

/// One shared live gate: a stored adoption alone is never current proof.
pub(crate) async fn adopted_receipt(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    snapshot: &str,
    consumer: &str,
    provider: &str,
    policy: CrossWorkstreamDependencyPolicy,
) -> PgResult<Option<String>> {
    let Some(r) = selection(tx, tenant, project, consumer, provider).await? else {
        return Ok(None);
    };
    let export: String = r.get(2);
    let checked = crate::cross_workstream_exports::validated_export(
        tx, tenant, project, snapshot, consumer, &export,
    )
    .await;
    let (manifest, export_version, disclosure) = match checked {
        Ok(v) => v,
        Err(error) if ordinary_invalid(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    if manifest["provider_work_id"] != provider
        || manifest["policy"] != json!(policy)
        || export_version != r.get::<_, i64>(3)
        || disclosure != r.get::<_, String>(4)
        || manifest["consumer_contract_hash"] != r.get::<_, String>(5)
        || manifest["consumer_ownership_version"] != r.get::<_, i64>(6).to_string()
    {
        return Ok(None);
    }
    Ok(manifest["receipt_id"].as_str().map(str::to_owned))
}

/// Completion must use the input receipts durably bound at execution admission,
/// not relabel an older result after selecting a different valid delivery.
pub(crate) async fn require_execution_inputs(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    consumer: &str,
    contract: &WorkContract,
    execution: Option<&str>,
    dependencies: &[(String, String)],
) -> PgResult<()> {
    if !contract
        .dependency_acceptance
        .values()
        .any(|mode| matches!(mode, DependencyAcceptanceMode::CrossWorkstream(_)))
    {
        return Ok(());
    }
    let execution = execution.ok_or(PgError::EvidenceInvalid)?;
    let rows = tx.query(
        "SELECT o.result_json,e.contract_hash,e.input_digest,e.ownership_version,e.workstream_id,e.session_id
         FROM awr_team.operations o JOIN awr_team.executions e
           ON e.tenant_id=o.tenant_id AND e.project_id=o.project_id
          AND e.executor_actor_id=o.actor_id AND e.executor_client_id=o.client_id
         WHERE o.tenant_id=$1 AND o.project_id=$2 AND e.id=$3 AND e.work_id=$4
           AND o.op='execution.start' AND o.state='committed'
           AND o.result_json->>'work_id'=$4 AND o.result_json->'data'->>'execution_id'=$3
         LIMIT 2",
        &[&tenant, &project, &execution, &consumer],
    ).await?;
    if rows.len() != 1 {
        return Err(PgError::EvidenceInvalid);
    }
    let r = &rows[0];
    let receipt: Value = r.get(0);
    let hash = contract.hash().map_err(|_| PgError::SourceDivergence)?;
    let ownership = r.get::<_, Option<i64>>(3).ok_or(PgError::EvidenceInvalid)?;
    if r.get::<_, String>(1) != hash
        || receipt["protocol"] != crate::workstream_command::RECEIPT_PROTOCOL
        || receipt["op"] != "execution.start"
        || receipt["contract_hash"] != hash
        || receipt["ownership_version"] != ownership.to_string()
        || receipt["workstream_id"] != json!(r.get::<_, Option<String>>(4))
        || receipt["data"]["session_id"] != json!(r.get::<_, Option<String>>(5))
        || receipt["data"]["input_digest"] != json!(r.get::<_, Option<String>>(2))
        || receipt["data"]["dependency_receipts"] != json!(dependencies)
    {
        return Err(PgError::EvidenceInvalid);
    }
    Ok(())
}

/// Bounded consumer-only selectors for preparation and adoption CAS.
pub(crate) async fn view(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    snapshot: &str,
    consumer: &str,
    contract: &WorkContract,
) -> PgResult<Vec<Value>> {
    let mut result = Vec::new();
    for provider in &contract.required_dependencies {
        let Some(DependencyAcceptanceMode::CrossWorkstream(policy)) =
            contract.dependency_acceptance.get(provider)
        else {
            continue;
        };
        let current = selection(tx, tenant, project, consumer, provider).await?;
        let receipt =
            adopted_receipt(tx, tenant, project, snapshot, consumer, provider, *policy).await?;
        result.push(json!({"provider_work_id":provider,"policy":policy,
            "adoption_version":current.as_ref().map(|r|r.get::<_,i64>(1)).unwrap_or(0).to_string(),
            "adoption_id":current.as_ref().map(|r|r.get::<_,String>(0)),
            "export_id":current.as_ref().map(|r|r.get::<_,String>(2)),
            "receipt_id":receipt,"valid":receipt.is_some(),
            "adoption_available":true}));
    }
    Ok(result)
}

pub(crate) async fn dependency_streams_match(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    snapshot: &str,
    stream: &str,
    contract: &WorkContract,
) -> PgResult<bool> {
    for upstream in &contract.required_dependencies {
        let r=tx.query_opt("SELECT workstream_id FROM awr_team.workstream_snapshot_ownership
            WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND scope_id='main' AND work_id=$4",
            &[&tenant,&project,&snapshot,upstream]).await?;
        let Some(r) = r else {
            return Ok(false);
        };
        let same = r.get::<_, String>(0) == stream;
        let cross = matches!(
            contract.dependency_acceptance.get(upstream),
            Some(DependencyAcceptanceMode::CrossWorkstream(_))
        );
        if same == cross {
            return Ok(false);
        }
    }
    Ok(true)
}
