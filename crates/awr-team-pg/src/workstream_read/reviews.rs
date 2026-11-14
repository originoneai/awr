//! Bounded review history from the same stored round and decision bindings as inspection.
use super::*;
use crate::workstream_command::reviews::{REVIEW_EVIDENCE_SCOPE_UNAVAILABLE, review_decision_role};

const RECORDS: &str = "WITH rounds AS (
    SELECT r.id,r.round_index,r.state,r.bundle_hash,r.contract_hash,r.artifact_digest,
        row_number() OVER (ORDER BY r.round_index,r.id) AS position,
        r.evidence_id IS NULL OR e.id IS NOT NULL AS readable,
        r.contract_hash=c.contract_hash AS contract_matches_current,
        d.id AS decision_id,d.decision,d.reviewer_actor_id,d.reviewer_person_id,
        d.approval_basis,d.reason,(extract(epoch FROM d.created_at)*1000)::bigint AS decided_at
    FROM awr_team.review_rounds r
    JOIN awr_team.work_contracts c ON c.tenant_id=r.tenant_id AND c.project_id=r.project_id
      AND c.snapshot_id=$4 AND c.scope_id='main' AND c.work_id=r.work_id
    LEFT JOIN awr_team.evidence e ON e.tenant_id=r.tenant_id AND e.project_id=r.project_id
      AND e.id=r.evidence_id AND e.work_id=r.work_id
    LEFT JOIN LATERAL (
      SELECT d.* FROM awr_team.review_decisions d
      WHERE d.tenant_id=r.tenant_id AND d.project_id=r.project_id AND d.review_round_id=r.id
        AND d.work_id=r.work_id AND d.bundle_hash=r.bundle_hash
      ORDER BY d.created_at DESC,d.id DESC LIMIT 1
    ) d ON true
    WHERE r.tenant_id=$1 AND r.project_id=$2 AND r.work_id=$3
)";

fn summary(row: &tokio_postgres::Row) -> Value {
    let readable: bool = row.get("readable");
    let role = row
        .get::<_, Option<String>>("approval_basis")
        .as_deref()
        .and_then(review_decision_role);
    let role = role.map(str::to_owned);
    let mut item = json!({
        "round_id":row.get::<_,String>("id"),"round_index":row.get::<_,i32>("round_index"),
        "round_position":row.get::<_,i64>("position"),"readable":readable,"reason":null,
        "state":row.get::<_,String>("state"),"bundle_hash":row.get::<_,String>("bundle_hash"),
        "contract_hash":row.get::<_,String>("contract_hash"),
        "contract_matches_current":row.get::<_,bool>("contract_matches_current"),
        "artifact_digest":row.get::<_,Option<String>>("artifact_digest"),
        "reviewer_role":role,"role_basis":if role.is_some(){Some("recorded_approval_basis")}else{None},
        "reviewer_actor_id":row.get::<_,Option<String>>("reviewer_actor_id"),
        "reviewer_person_id":row.get::<_,Option<String>>("reviewer_person_id"),
        "decision":row.get::<_,Option<String>>("decision"),
        "decided_at_unix_ms":row.get::<_,Option<i64>>("decided_at"),
        "reason_first_line":row.get::<_,Option<String>>("reason").map(|reason| reason.lines().next().unwrap_or("").to_owned()),
        "missing":{}
    });
    for field in [
        "state",
        "bundle_hash",
        "contract_hash",
        "contract_matches_current",
        "artifact_digest",
        "reviewer_role",
        "role_basis",
        "reviewer_actor_id",
        "reviewer_person_id",
        "decision",
        "decided_at_unix_ms",
        "reason_first_line",
    ] {
        if !readable {
            item[field] = Value::Null;
            item["missing"][field] = json!(REVIEW_EVIDENCE_SCOPE_UNAVAILABLE);
        } else {
            item["missing"][field] = if item[field].is_null() {
                json!("not_recorded")
            } else {
                Value::Null
            };
        }
    }
    if !readable {
        item["reason"] = json!(REVIEW_EVIDENCE_SCOPE_UNAVAILABLE);
    }
    item
}

pub(super) async fn read(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    work: &str,
    q: &WorkstreamQuery,
) -> PgResult<Value> {
    let params: &[&(dyn tokio_postgres::types::ToSql + Sync)] =
        &[&tenant, &project, &work, &auth.snapshot];
    let stamp = tx.query_one(&format!("{RECORDS}
        SELECT count(*),md5(COALESCE(string_agg(jsonb_build_array(id,round_index,readable,
          CASE WHEN readable THEN jsonb_build_array(state,bundle_hash,contract_hash,artifact_digest,
            contract_matches_current,decision_id,decision,reviewer_actor_id,reviewer_person_id,
            approval_basis,md5(reason),decided_at) ELSE NULL END)::text,E'\\n' ORDER BY round_index,id),'')),
          (extract(epoch FROM statement_timestamp())*1000)::bigint FROM rounds"),params).await?;
    let revision: String = stamp.get(1);
    let binding = hash(&json!({"domain":"awr-review-rounds-v1","auth":auth.binding,
        "snapshot":auth.snapshot,"epoch":auth.epoch,"work":work,"revision":revision}))?;
    let cursor = cursor(q, &binding)?;
    if !cursor.key.is_empty() || cursor.index < -1 {
        return Err(PgError::CursorExpired);
    }
    let offset = i64::from(cursor.index) + 1;
    let total: i64 = stamp.get(0);
    if offset > total {
        return Err(PgError::CursorExpired);
    }
    let limit = i64::from(q.limit.unwrap_or(20));
    let page_limit = limit + 1;
    let mut page_params = params.to_vec();
    page_params.extend([
        &offset as &(dyn tokio_postgres::types::ToSql + Sync),
        &page_limit,
    ]);
    let rows = tx
        .query(
            &format!("{RECORDS} SELECT * FROM rounds ORDER BY round_index,id OFFSET $5 LIMIT $6"),
            &page_params,
        )
        .await?;
    let more = rows.len() > limit as usize;
    let items: Vec<_> = rows.iter().take(limit as usize).map(summary).collect();
    let next = if more {
        next_cursor(
            &binding,
            "",
            0,
            i32::try_from(offset + limit - 1).map_err(|_| PgError::ResponseTooLarge)?,
        )
    } else {
        Value::Null
    };
    Ok(
        json!({"work_id":work,"items":items,"total":total,"next_cursor":next,"truncated":more,
        "execution_authorized":false,
        "as_of":{"at_unix_ms":stamp.get::<_,i64>(2),"source_snapshot_id":auth.snapshot,
            "project_revision":auth.revision.to_string(),"rounds_revision":revision}}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_roles_follow_recorded_decisions_without_inventing_membership_roles() {
        assert_eq!(
            review_decision_role("human_independent_review"),
            Some("independent_reviewer")
        );
        assert_eq!(review_decision_role("agent_review"), Some("agent_reviewer"));
        assert_eq!(review_decision_role("future_basis"), None);
    }
}
