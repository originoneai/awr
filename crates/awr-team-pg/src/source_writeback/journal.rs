//! Durable phases retain one immutable original intent across crashes.
use super::*;
use request_binding::Intent;

pub(super) struct Journal {
    pub phase: String,
    pub intent: Intent,
    pub hash: String,
    pub receipt_id: Option<String>,
}

pub(super) async fn load(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    request: &str,
) -> PgResult<Option<Journal>> {
    let row = tx.query_opt(
        "SELECT phase,intent_hash,intent_json,audit_receipt_id,candidate_id,candidate_digest,publish_receipt_id,
                before_fingerprint,after_fingerprint,affected_work_ids,dependency_work_ids
         FROM awr_team.planning_writeback_journals WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3 FOR UPDATE",
        &[&tenant,&project,&request],
    ).await?;
    let Some(row) = row else { return Ok(None) };
    parse(&row, tenant, project, request).map(Some)
}

fn parse(
    row: &tokio_postgres::Row,
    tenant: &str,
    project: &str,
    request: &str,
) -> PgResult<Journal> {
    let hash: String = row.get::<_, Option<String>>(1).ok_or_else(unknown)?;
    let intent: Intent =
        serde_json::from_value(row.get::<_, Option<Value>>(2).ok_or_else(unknown)?)
            .map_err(|_| unknown())?;
    if intent.codec != "awr-planning-writeback-intent-v1"
        || intent.hash()? != hash
        || intent.request.request_id != request
        || intent.tenant_id != tenant
        || intent.project_id != project
        || intent.publication.candidate_id != row.get::<_, String>(4)
        || intent.publication.candidate_digest != row.get::<_, String>(5)
        || intent.request.publish_receipt_id != row.get::<_, String>(6)
        || intent.before_fingerprint != row.get::<_, String>(7)
        || intent.after_fingerprint != row.get::<_, String>(8)
        || json!(intent.affected_work_ids) != row.get::<_, Value>(9)
        || json!(intent.dependency_work_ids) != row.get::<_, Value>(10)
    {
        return Err(PgError::SourceDivergence);
    };
    Ok(Journal {
        phase: row.get(0),
        intent,
        hash,
        receipt_id: row.get(3),
    })
}

fn unknown() -> PgError {
    PgError::WritebackRefused("original writeback intent is unknown; inspect the retained source and journal without inferring authority".into())
}

pub(super) async fn insert(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    intent: &Intent,
    gate: &ActivationImpactGate,
    phase: &str,
) -> PgResult<()> {
    let p = &intent.publication;
    tx.execute(
        "INSERT INTO awr_team.planning_writeback_journals(
         tenant_id,project_id,request_id,candidate_id,candidate_digest,publish_receipt_id,phase,
         before_fingerprint,after_fingerprint,approver_actor_id,publisher_actor_id,affected_work_ids,
         unrelated_work_ids,recovery_actions,refuse_reason,body_json,intent_hash,intent_json,dependency_work_ids)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19)",
        &[&tenant,&project,&intent.request.request_id,&p.candidate_id,&p.candidate_digest,&intent.request.publish_receipt_id,&phase,
          &intent.before_fingerprint,&intent.after_fingerprint,&p.approver_actor_id,&p.publisher_actor_id,&json!(intent.affected_work_ids),
          &json!(gate.unrelated_work_ids),&json!(gate.recovery_actions),&gate.refuse_reason,&json!({"phase":phase}),
          &intent.hash()?,&json!(intent),&json!(intent.dependency_work_ids)],
    ).await?;
    Ok(())
}

pub(super) async fn transition(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    request: &str,
    hash: &str,
    phase: &str,
) -> PgResult<()> {
    let previous = match phase {
        "source_written" => "validated",
        "pg_activating" => "source_written",
        _ => {
            return Err(PgError::Protocol(
                "invalid writeback phase transition".into(),
            ));
        }
    };
    if tx.execute(
        "UPDATE awr_team.planning_writeback_journals SET phase=$5,body_json=jsonb_build_object('phase',$5::text),updated_at=clock_timestamp()
         WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3 AND intent_hash=$4 AND phase=$6",
        &[&tenant,&project,&request,&hash,&phase,&previous],
    ).await? != 1 {return Err(PgError::PreconditionsChanged)};
    Ok(())
}

pub(crate) async fn outcome_in_tx(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    request: &str,
) -> PgResult<Option<Value>> {
    let row = tx.query_opt(
        "SELECT phase,intent_hash,intent_json,audit_receipt_id,candidate_id,candidate_digest,publish_receipt_id,
                before_fingerprint,after_fingerprint,affected_work_ids,dependency_work_ids
         FROM awr_team.planning_writeback_journals
         WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3 FOR SHARE", &[&tenant,&project,&request],
    ).await?;
    let Some(row) = row else { return Ok(None) };
    let phase: String = row.get(0);
    // A phase name or non-null hash cannot establish original provenance.
    // Unknown legacy and corrupt rows remain observable, never upgraded.
    let saved = match parse(&row, tenant, project, request) {
        Ok(saved) => Some(saved),
        Err(PgError::WritebackRefused(_) | PgError::SourceDivergence) => None,
        Err(error) => return Err(error),
    };
    let bound = saved.is_some();
    let applied = if let Some(saved) = saved.as_ref().filter(|s| s.phase == "completed") {
        let intent = &saved.intent;
        let publication = &intent.publication;
        let snapshot = intent.candidate.as_ref().map(|c| c.snapshot_id.clone());
        let manifest = intent.candidate.as_ref().map(|c| c.manifest_digest.clone());
        let parser = intent.candidate.as_ref().map(|c| c.parser_version.clone());
        tx.query_one(
            "SELECT EXISTS(SELECT 1 FROM awr_team.planning_activation_receipts r
             JOIN awr_team.planning_publish_receipts p
               ON p.tenant_id=r.tenant_id AND p.project_id=r.project_id AND p.id=r.publish_receipt_id
             JOIN awr_team.source_snapshots s
               ON s.tenant_id=r.tenant_id AND s.project_id=r.project_id AND s.id=r.activated_snapshot_id
             WHERE r.tenant_id=$1 AND r.project_id=$2 AND r.request_id=$3 AND r.id=$4
               AND r.candidate_id=$5 AND r.candidate_digest=$6 AND r.publish_receipt_id=$7
               AND r.before_fingerprint=$8 AND r.after_fingerprint=$9 AND r.source_version=$10
               AND r.audit_json->>'intent_hash'=$11 AND r.approval_id=$12
               AND r.activated_snapshot_id=$13 AND NOT p.source_writeback_pending
               AND p.activation_receipt_id=r.id AND p.activated_snapshot_id=r.activated_snapshot_id
               AND p.source_version=r.source_version
               AND p.candidate_id=r.candidate_id AND p.candidate_digest=r.candidate_digest
               AND p.approval_id=r.approval_id
               AND s.manifest_digest=$14 AND s.parser_version=$15)",
            &[&tenant,&project,&request,&saved.receipt_id,&publication.candidate_id,&publication.candidate_digest,
              &intent.request.publish_receipt_id,&intent.before_fingerprint,&intent.after_fingerprint,
              &intent.source_version,&saved.hash,&publication.approval_id,&snapshot,&manifest,&parser],
        ).await?.get(0)
    } else {
        false
    };
    Ok(Some(
        json!({"request_id":request,"phase":phase,"original_intent_bound":bound,
        "pending":matches!(phase.as_str(),"validated"|"source_written"|"pg_activating"),
        "source_writeback_pending":!applied,"applied":applied,
        "activation_receipt_id":if applied {row.get::<_,Option<String>>(3)} else {None},
        "next_step":if !bound {"original intent is unknown or inconsistent; inspect retained facts without inferring authority"}
            else if phase=="completed" && !applied {"activation confirmation is inconsistent; inspect retained source and receipts before recovery"}
            else if applied {"reuse the original receipt; do not activate again"}
            else {"inspect the source and resume the same original request; pending is not applied"}}),
    ))
}
