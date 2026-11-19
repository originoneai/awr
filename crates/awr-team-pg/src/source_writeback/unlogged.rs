//! Owner recovery for a committed publication whose reserved activation has no journal.
//! The publication, candidate and original command receipt are closed atomically;
//! neither authoritative source bytes nor an activated snapshot are changed.
use super::operator::OperatorWriteback;
use super::{PgError, PgResult, Value, json};
use crate::operator_access::require_owner_project;
use tokio_postgres::{Client, Row, Transaction};

const ACTION: &str = "close_unlogged";
const PROTOCOL: &str = super::journal::OPERATOR_DECISION_PROTOCOL;
const LIMIT: i64 = 50;

#[derive(Clone, Debug)]
pub struct UnloggedCloseRequest {
    pub request_id: String,
    pub expected_candidate_id: String,
    pub expected_candidate_digest: String,
    pub expected_publish_receipt_id: String,
    pub expected_request_hash: String,
    pub reason: String,
}

fn invalid() -> PgError {
    PgError::Protocol("invalid operator unlogged writeback close request".into())
}

fn identity(value: &str) -> bool {
    !value.is_empty() && value.len() <= 200 && !value.chars().any(char::is_control)
}

fn hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_reason(reason: &str) -> bool {
    !reason.trim().is_empty()
        && reason.chars().count() <= super::journal::MAX_OPERATOR_REASON
        && !reason.chars().any(char::is_control)
}

fn row_item(row: &Row) -> Value {
    let request_id = row.get::<_, String>(0);
    let request_hash = row.get::<_, String>(1);
    let candidate_id = row.get::<_, String>(3);
    let candidate_digest = row.get::<_, String>(4);
    let publish_receipt_id = row.get::<_, String>(2);
    let complete_identity = hex(&request_hash)
        && hex(&candidate_digest)
        && identity(&request_id)
        && identity(&candidate_id)
        && identity(&publish_receipt_id);
    json!({
        "kind":"publication_without_journal",
        "request_id":request_id,
        "candidate_id":candidate_id,
        "candidate_digest":candidate_digest,
        "publish_receipt_id":publish_receipt_id,
        "created_at":row.get::<_,String>(6),
        "request_created_at":row.get::<_,String>(5),
        "published_at":row.get::<_,String>(6),
        "source_writeback_pending":true,
        "writeback":"absent",
        "close_unlogged":{
            "allowed":complete_identity,
            "expected_candidate_id":candidate_id,
            "expected_candidate_digest":candidate_digest,
            "expected_publish_receipt_id":publish_receipt_id,
            "expected_request_hash":request_hash
        },
        "next_step":"Retry the identical planning.publish request once. If it still cannot create a writeback journal, the schema owner can close this unactivated publication with writeback-close-unlogged and these expected_* values."
    })
}

const SELECT: &str = "SELECT r.request_id,r.request_hash,p.id,p.candidate_id,p.candidate_digest,
        r.created_at::text,p.created_at::text
     FROM awr_team.planning_command_receipts r
     JOIN awr_team.planning_publish_receipts p
       ON p.tenant_id=r.tenant_id AND p.project_id=r.project_id
      AND p.candidate_id=r.result_json->'canonical_request'->>'candidate_id'
     JOIN awr_team.planning_candidates c
       ON c.tenant_id=p.tenant_id AND c.project_id=p.project_id AND c.id=p.candidate_id
      AND c.state='published' AND c.candidate_digest=p.candidate_digest
     WHERE r.tenant_id=$1 AND r.project_id=$2 AND r.op='planning.activate'
       AND r.status='reserved' AND p.source_writeback_pending
       AND p.activation_receipt_id IS NULL AND p.activated_snapshot_id IS NULL
       AND (r.result_json->'canonical_request'->>'publish_receipt_id' IS NULL
            OR r.result_json->'canonical_request'->>'publish_receipt_id'=p.id)
       AND NOT EXISTS(SELECT 1 FROM awr_team.planning_writeback_journals j
                      WHERE j.tenant_id=r.tenant_id AND j.project_id=r.project_id
                        AND j.request_id=r.request_id)";

pub(super) async fn list(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    request: Option<&str>,
) -> PgResult<Vec<Value>> {
    let rows = if let Some(request) = request {
        tx.query(
            &format!("{SELECT} AND r.request_id=$3"),
            &[&tenant, &project, &request],
        )
        .await?
    } else {
        tx.query(
            &format!("{SELECT} ORDER BY p.created_at,r.request_id LIMIT $3"),
            &[&tenant, &project, &(LIMIT + 1)],
        )
        .await?
    };
    Ok(rows.iter().map(row_item).collect())
}

/// Recognize only the complete owner decision previously stored in this request receipt.
pub(crate) fn recorded_decision<'a>(body: &'a Value, request: &str) -> Option<&'a Value> {
    let decision = body.get("result")?.get("operator_closure")?;
    let valid = body.get("protocol").and_then(Value::as_str)
        == Some(super::super::planning_ops::RECEIPT_PROTOCOL)
        && body.get("op").and_then(Value::as_str) == Some("planning.activate")
        && body.get("request_id").and_then(Value::as_str) == Some(request)
        && decision.get("protocol").and_then(Value::as_str) == Some(PROTOCOL)
        && decision.get("action").and_then(Value::as_str) == Some(ACTION)
        && decision.get("request_id").and_then(Value::as_str) == Some(request)
        && decision.get("activated").and_then(Value::as_bool) == Some(false)
        && decision
            .get("request_hash")
            .and_then(Value::as_str)
            .is_some_and(|hash| {
                hex(hash) && body.get("request_hash").and_then(Value::as_str) == Some(hash)
            })
        && decision
            .get("candidate_id")
            .and_then(Value::as_str)
            .is_some_and(identity)
        && decision
            .get("candidate_digest")
            .and_then(Value::as_str)
            .is_some_and(hex)
        && decision
            .get("publish_receipt_id")
            .and_then(Value::as_str)
            .is_some_and(identity)
        && decision
            .get("operator_role")
            .and_then(Value::as_str)
            .is_some_and(identity)
        && decision
            .get("reason")
            .and_then(Value::as_str)
            .is_some_and(valid_reason);
    valid.then_some(decision)
}

fn same_reviewed_facts(req: &UnloggedCloseRequest, decision: &Value) -> bool {
    decision.get("candidate_id").and_then(Value::as_str) == Some(&req.expected_candidate_id)
        && decision.get("candidate_digest").and_then(Value::as_str)
            == Some(&req.expected_candidate_digest)
        && decision.get("publish_receipt_id").and_then(Value::as_str)
            == Some(&req.expected_publish_receipt_id)
        && decision.get("request_hash").and_then(Value::as_str) == Some(&req.expected_request_hash)
        && decision.get("reason").and_then(Value::as_str) == Some(req.reason.trim())
}

impl OperatorWriteback {
    /// Close the exact no-journal activation previewed by `inspect`, or replay its stored decision.
    pub async fn close_unlogged(
        client: &mut Client,
        tenant: &str,
        project: &str,
        req: &UnloggedCloseRequest,
    ) -> PgResult<Value> {
        let reason = req.reason.trim();
        if ![
            tenant,
            project,
            &req.request_id,
            &req.expected_candidate_id,
            &req.expected_publish_receipt_id,
        ]
        .into_iter()
        .all(|value| identity(value))
            || !hex(&req.expected_candidate_digest)
            || !hex(&req.expected_request_hash)
            || !valid_reason(reason)
        {
            return Err(invalid());
        }
        crate::check_schema(client).await?;
        // The original request is locked before its publication, matching normal
        // planning retry order. A direct writer holds the publication before its
        // journal insert, so this close cannot miss a committed journal.
        let tx = client.transaction().await?;
        let operator = require_owner_project(&tx, tenant, project, true).await?;
        let receipt = tx
            .query_opt(
                "SELECT op,request_hash,status,result_json
                 FROM awr_team.planning_command_receipts
                 WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3 FOR UPDATE",
                &[&tenant, &project, &req.request_id],
            )
            .await?
            .ok_or_else(|| {
                PgError::WritebackRefused(
                    "no planning activation request exists for this request".into(),
                )
            })?;
        let body: Value = receipt.get(3);
        if receipt.get::<_, String>(2) == "completed" {
            let Some(decision) = recorded_decision(&body, &req.request_id) else {
                return Err(PgError::WritebackRefused(
                    "the completed planning activation was not closed by this owner operation"
                        .into(),
                ));
            };
            if !same_reviewed_facts(req, decision) {
                return Err(PgError::PreconditionsChanged);
            }
            let journal: bool = tx
                .query_one(
                    "SELECT EXISTS(SELECT 1 FROM awr_team.planning_writeback_journals
                     WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3)",
                    &[&tenant, &project, &req.request_id],
                )
                .await?
                .get(0);
            if journal {
                return Err(PgError::WritebackRefused(
                    "a writeback journal exists; use writeback-abandon for its inspected phase"
                        .into(),
                ));
            }
            let decision = decision.clone();
            tx.commit().await?;
            return Ok(json!({"protocol":PROTOCOL,"replayed":true,"decision":decision}));
        }
        if receipt.get::<_, String>(0) != "planning.activate"
            || receipt.get::<_, String>(2) != "reserved"
        {
            return Err(PgError::WritebackRefused(
                "the original planning activation request is not reserved".into(),
            ));
        }
        if receipt.get::<_, String>(1) != req.expected_request_hash {
            return Err(PgError::PreconditionsChanged);
        }
        let canonical = body.get("canonical_request").ok_or_else(|| {
            PgError::WritebackRefused("the reserved activation has no canonical request".into())
        })?;
        if canonical.get("candidate_id").and_then(Value::as_str)
            != Some(req.expected_candidate_id.as_str())
            || canonical.get("candidate_digest").and_then(Value::as_str)
                != Some(req.expected_candidate_digest.as_str())
            || canonical.get("activate").and_then(Value::as_bool) != Some(true)
            || canonical
                .get("publish_receipt_id")
                .and_then(Value::as_str)
                .is_some_and(|id| id != req.expected_publish_receipt_id)
        {
            return Err(PgError::PreconditionsChanged);
        }
        let publication = tx
            .query_opt(
                "SELECT candidate_id,candidate_digest,source_writeback_pending,
                        activation_receipt_id,activated_snapshot_id
                 FROM awr_team.planning_publish_receipts
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3 FOR UPDATE",
                &[&tenant, &project, &req.expected_publish_receipt_id],
            )
            .await?
            .ok_or_else(|| {
                PgError::WritebackRefused("the inspected publication no longer exists".into())
            })?;
        if publication.get::<_, String>(0) != req.expected_candidate_id
            || publication.get::<_, String>(1) != req.expected_candidate_digest
        {
            return Err(PgError::PreconditionsChanged);
        }
        if !publication.get::<_, bool>(2)
            || publication.get::<_, Option<String>>(3).is_some()
            || publication.get::<_, Option<String>>(4).is_some()
        {
            return Err(PgError::WritebackRefused(
                "the publication is activated or no longer waiting for writeback".into(),
            ));
        }
        let journal: bool = tx
            .query_one(
                "SELECT EXISTS(SELECT 1 FROM awr_team.planning_writeback_journals
                 WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3)",
                &[&tenant, &project, &req.request_id],
            )
            .await?
            .get(0);
        if journal {
            return Err(PgError::WritebackRefused(
                "a writeback journal exists; use writeback-abandon for its inspected phase".into(),
            ));
        }
        let candidate = tx
            .query_opt(
                "SELECT state,candidate_digest FROM awr_team.planning_candidates
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3 FOR UPDATE",
                &[&tenant, &project, &req.expected_candidate_id],
            )
            .await?
            .ok_or_else(|| {
                PgError::WritebackRefused("the planning candidate no longer exists".into())
            })?;
        if candidate.get::<_, String>(0) != "published"
            || candidate.get::<_, String>(1) != req.expected_candidate_digest
        {
            return Err(PgError::PreconditionsChanged);
        }
        let decision = json!({
            "protocol":PROTOCOL,"action":ACTION,"request_id":req.request_id,
            "request_hash":req.expected_request_hash,"candidate_id":req.expected_candidate_id,
            "candidate_digest":req.expected_candidate_digest,
            "publish_receipt_id":req.expected_publish_receipt_id,
            "operator_role":operator,"reason":reason,"activated":false
        });
        let result = json!({
            "protocol":super::super::planning_ops::RECEIPT_PROTOCOL,
            "request_id":req.request_id,"op":"planning.activate",
            "request_hash":req.expected_request_hash,
            "result":{
                "status":"closed","writeback":"absent","operator_closure":decision,
                "next_step":"The schema owner closed this unactivated publication. Review the retained reason and create a new reviewed planning request instead of retrying this one."
            },
            "already_recorded":false
        });
        let closed_publication = tx
            .execute(
                "UPDATE awr_team.planning_publish_receipts
                 SET source_writeback_pending=false
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3
                   AND source_writeback_pending AND activation_receipt_id IS NULL
                   AND activated_snapshot_id IS NULL",
                &[&tenant, &project, &req.expected_publish_receipt_id],
            )
            .await?;
        let closed_candidate = tx
            .execute(
                "UPDATE awr_team.planning_candidates SET state='superseded',updated_at=clock_timestamp()
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3
                   AND state='published' AND candidate_digest=$4",
                &[
                    &tenant,
                    &project,
                    &req.expected_candidate_id,
                    &req.expected_candidate_digest,
                ],
            )
            .await?;
        let completed_request = tx
            .execute(
                "UPDATE awr_team.planning_command_receipts
                 SET status='completed',result_json=$4,updated_at=clock_timestamp()
                 WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3
                   AND op='planning.activate' AND status='reserved' AND request_hash=$5",
                &[
                    &tenant,
                    &project,
                    &req.request_id,
                    &result,
                    &req.expected_request_hash,
                ],
            )
            .await?;
        if closed_publication != 1 || closed_candidate != 1 || completed_request != 1 {
            return Err(PgError::PreconditionsChanged);
        }
        tx.commit().await?;
        Ok(json!({
            "protocol":PROTOCOL,"replayed":false,"decision":decision,
            "publication_closed":true,"candidate_state":"superseded",
            "request_status":"completed","source_modified":false,
            "active_snapshot_modified":false,"activated":false
        }))
    }
}
