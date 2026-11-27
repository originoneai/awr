//! Bounded review history from the same stored round and decision bindings as inspection.
use super::*;
use crate::workstream_command::reviews::{REVIEW_EVIDENCE_SCOPE_UNAVAILABLE, review_decision_role};

const RECORDS: &str = "WITH rounds AS (
    SELECT r.id,r.round_index,r.state,r.bundle_hash,r.contract_hash,r.artifact_digest,
        COALESCE(r.requested_reviewer, jsonb_build_object('kind','role','role','reviewer')) AS requested_reviewer,
        (extract(epoch FROM r.created_at)*1000)::bigint AS opened_at_unix_ms,
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
        "requested_reviewer":row.get::<_,Value>("requested_reviewer"),
        "opened_at_unix_ms":row.get::<_,i64>("opened_at_unix_ms"),
        "state":row.get::<_,String>("state"),"bundle_hash":row.get::<_,String>("bundle_hash"),
        "contract_hash":row.get::<_,String>("contract_hash"),
        "contract_matches_current":row.get::<_,bool>("contract_matches_current"),
        "artifact_digest":row.get::<_,Option<String>>("artifact_digest"),
        "reviewer_role":role,"role_basis":if role.is_some(){Some("recorded_approval_basis")}else{None},
        "reviewer_actor_id":row.get::<_,Option<String>>("reviewer_actor_id"),
        "reviewer_person_id":row.get::<_,Option<String>>("reviewer_person_id"),
        "decision":row.get::<_,Option<String>>("decision"),
        "decided_at_unix_ms":row.get::<_,Option<i64>>("decided_at"),
        "reason_first_line":row.get::<_,Option<String>>("reason").map(|reason| reason.split(['\r','\n','\u{0085}','\u{2028}','\u{2029}']).next().unwrap_or("").to_owned()),
        "missing":{}
    });
    for field in [
        "state",
        "requested_reviewer",
        "opened_at_unix_ms",
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
    read_binding: &str,
) -> PgResult<Value> {
    let params: &[&(dyn tokio_postgres::types::ToSql + Sync)] =
        &[&tenant, &project, &work, &auth.snapshot];
    let stamp = tx.query_one(&format!("{RECORDS}
        SELECT count(*),md5(COALESCE(string_agg(jsonb_build_array(id,round_index,readable,
          CASE WHEN readable THEN jsonb_build_array(state,bundle_hash,contract_hash,artifact_digest,
            contract_matches_current,requested_reviewer,opened_at_unix_ms,decision_id,decision,reviewer_actor_id,reviewer_person_id,
            approval_basis,md5(reason),decided_at) ELSE NULL END)::text,E'\\n' ORDER BY round_index,id),'')),
          (extract(epoch FROM statement_timestamp())*1000)::bigint FROM rounds"),params).await?;
    let revision: String = stamp.get(1);
    let binding = hash(
        &json!({"domain":"awr-review-rounds-v1","authority":read_binding,
        "snapshot":auth.snapshot,"epoch":auth.epoch,"work":work,"revision":revision}),
    )?;
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
    fn queue_query_selectors_are_bounded_and_operation_specific() {
        for op in ["review.queue", "team.reviewers"] {
            let q: WorkstreamQuery = serde_json::from_value(
                json!({"protocol_version":1,"op":op,"limit":10,"max_context_bytes":65536}),
            )
            .unwrap();
            q.validate().unwrap();
            for field in ["session_id", "review_round_id"] {
                let q: WorkstreamQuery = serde_json::from_value(
                    json!({"protocol_version":1,"op":op,field:"unexpected"}),
                )
                .unwrap();
                assert!(q.validate().is_err());
            }
        }
        let q: WorkstreamQuery = serde_json::from_value(
            json!({"protocol_version":1,"op":"review.queue","recipient_person_id":"reviewer"}),
        )
        .unwrap();
        q.validate().unwrap();
        let q: WorkstreamQuery = serde_json::from_value(
            json!({"protocol_version":1,"op":"team.reviewers","recipient_person_id":"reviewer"}),
        )
        .unwrap();
        assert!(q.validate().is_err());
    }

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

/// Current review duty, derived from the same membership ceiling as review admission.
/// This discovery grants neither review permission nor independent acceptance.
pub(crate) async fn reviewers(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
) -> PgResult<std::collections::BTreeMap<String, String>> {
    let rows = tx
        .query(
            "SELECT m.actor_id,m.role,a.kind,m.independent_review,m.agent_review,
        m.business_roles,m.assignment_grant FROM awr_team.project_memberships m
        JOIN awr_team.actors a ON a.tenant_id=m.tenant_id AND a.id=m.actor_id
        WHERE m.tenant_id=$1 AND m.project_id=$2 AND a.status='active' ORDER BY m.actor_id",
            &[&tenant, &project],
        )
        .await?;
    let ids: Vec<String> = rows.iter().map(|r| r.get(0)).collect();
    let people = crate::review::resolve_person_ids(tx, tenant, project, &ids).await?;
    let mut result = std::collections::BTreeMap::new();
    for row in rows {
        let role: String = row.get(1);
        let kind: String = row.get(2);
        let duties = crate::workstream_auth::decode_business_roles(row.get(5))?;
        let ceiling = crate::workstream_auth::membership_action_ceiling(
            &role,
            crate::workstream_auth::map_membership_role(&role).ok_or(PgError::Forbidden)?,
            &kind,
            row.get(3),
            row.get(4),
            row.get(6),
            duties.as_ref(),
        );
        if ceiling.contains(&awr_team::Action::ReviewDecide) {
            if let Some((id, name)) = people
                .get(&row.get::<_, String>(0))
                .and_then(|p| p.active_person())
            {
                result.insert(id.to_owned(), name.to_owned());
            }
        }
    }
    Ok(result)
}

const QUEUE_RECORDS: &str = "WITH records AS (
    SELECT r.id AS round_id,r.work_id,c.title AS work_title,r.round_index,
      r.contract_hash=c.contract_hash AS contract_matches_current,
      COALESCE(r.requested_reviewer,'{\"kind\":\"role\",\"role\":\"reviewer\"}'::jsonb) AS requested_reviewer,
      (extract(epoch FROM r.created_at)*1000)::bigint AS opened_at_unix_ms,
      greatest(0,(extract(epoch FROM transaction_timestamp()-r.created_at)*1000)::bigint) AS wait_ms
    FROM awr_team.review_rounds r
    JOIN awr_team.work_contracts c ON c.tenant_id=r.tenant_id AND c.project_id=r.project_id
      AND c.snapshot_id=$3 AND c.scope_id='main' AND c.work_id=r.work_id
    JOIN awr_team.workstream_snapshot_ownership o ON o.tenant_id=c.tenant_id AND o.project_id=c.project_id
      AND o.snapshot_id=c.snapshot_id AND o.scope_id=c.scope_id AND o.work_id=c.work_id
    WHERE r.tenant_id=$1 AND r.project_id=$2 AND r.state='open'
      AND c.definition_state='enabled'
      AND (o.workstream_id=ANY($4) OR EXISTS (
        SELECT 1 FROM unnest($5::text[],$6::text[]) allowed(work_id,workstream_id)
        WHERE allowed.work_id=r.work_id AND allowed.workstream_id=o.workstream_id))
      AND ($7::text IS NULL OR r.work_id=$7) AND ($8::text IS NULL OR o.workstream_id=$8)
      AND (r.evidence_id IS NULL OR EXISTS (SELECT 1 FROM awr_team.evidence e
        WHERE e.tenant_id=r.tenant_id AND e.project_id=r.project_id AND e.id=r.evidence_id AND e.work_id=r.work_id))
      AND ($9::text IS NULL OR r.requested_reviewer->>'person_id'=$9
        OR COALESCE(r.requested_reviewer->>'kind','role')='role')
)";

pub(super) async fn queue(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    q: &WorkstreamQuery,
) -> PgResult<Value> {
    let bound = if let Some(work) = &q.work_id {
        let (binding, _) = work_binding(tx, tenant, project, auth, work).await?;
        if q.workstream_id
            .is_some_and(|id| id != binding.workstream_id)
        {
            return Err(PgError::Forbidden);
        }
        Some(binding.workstream_id)
    } else {
        q.workstream_id
    };
    if let Some(stream) = bound {
        auth.access
            .authorize(&auth.catalog, stream, WorkstreamAction::Read)
            .map_err(|_| PgError::Forbidden)?;
    }
    let roster = reviewers(tx, tenant, project).await?;
    // An explicit person selector is a project member with the review duty.
    if q.recipient_person_id
        .as_ref()
        .is_some_and(|id| !roster.contains_key(id))
    {
        return Err(PgError::Forbidden);
    }
    let (streams, tasks, task_streams) = navigation::visible_work_scope(auth);
    let stream = bound.map(|id| id.to_string());
    let params: &[&(dyn tokio_postgres::types::ToSql + Sync)] = &[
        &tenant,
        &project,
        &auth.snapshot,
        &streams,
        &tasks,
        &task_streams,
        &q.work_id,
        &stream,
        &q.recipient_person_id,
    ];
    let stamp = tx.query_one(&format!("{QUEUE_RECORDS} SELECT count(*),
        md5(COALESCE(string_agg(jsonb_build_array(round_id,work_id,work_title,round_index,
          contract_matches_current,requested_reviewer,opened_at_unix_ms)::text,E'\\n' ORDER BY opened_at_unix_ms,round_id),'')),
        (extract(epoch FROM transaction_timestamp())*1000)::bigint FROM records"),params).await?;
    let revision: String = stamp.get(1);
    let binding = hash(
        &json!({"domain":"awr-review-queue-v1","op":q.op,"reader":auth.binding,
        "snapshot":auth.snapshot,"epoch":auth.epoch,"grants":auth.grant_versions,"scope":auth.navigation_read_scope,
        "work":q.work_id,"stream":stream,"recipient":q.recipient_person_id,"revision":revision,"roster":roster}),
    )?;
    let c = cursor(q, &binding)?;
    if !c.key.is_empty() || c.index < -1 {
        return Err(PgError::CursorExpired);
    }
    let offset = i64::from(c.index) + 1;
    let limit = i64::from(q.limit.unwrap_or(20));
    let aggregates = tx
        .query(
            &format!(
                "{QUEUE_RECORDS} SELECT requested_reviewer,count(*),max(wait_ms)
        FROM records GROUP BY requested_reviewer ORDER BY requested_reviewer::text"
            ),
            params,
        )
        .await?;
    let groups: Vec<_> = aggregates
        .iter()
        .map(|r| {
            json!({"requested_reviewer":r.get::<_,Value>(0),
        "queue_count":r.get::<_,i64>(1),"oldest_wait_ms":r.get::<_,i64>(2)})
        })
        .collect();
    let total: i64;
    let items: Vec<Value>;
    if q.op == "team.reviewers" {
        total = roster.len() as i64;
        items = roster
            .iter()
            .skip(offset as usize)
            .take(limit as usize)
            .map(|(id, name)| {
                let count: i64 = groups
                    .iter()
                    .filter(|g| {
                        g["requested_reviewer"]["kind"] == "role"
                            || g["requested_reviewer"]["person_id"].as_str() == Some(id.as_str())
                    })
                    .map(|g| g["queue_count"].as_i64().unwrap())
                    .sum();
                json!({"name":name,"review_duty":"reviewer","queue_count":count})
            })
            .collect();
    } else {
        total = stamp.get(0);
        let page_limit = limit + 1;
        let mut page_params = params.to_vec();
        page_params.extend([
            &offset as &(dyn tokio_postgres::types::ToSql + Sync),
            &page_limit,
        ]);
        let rows=tx.query(&format!("{QUEUE_RECORDS} SELECT * FROM records ORDER BY opened_at_unix_ms,round_id OFFSET $10 LIMIT $11"),&page_params).await?;
        items=rows.iter().take(limit as usize).map(|r| json!({
            "round_id":r.get::<_,String>("round_id"),"work_id":r.get::<_,String>("work_id"),
            "work_title":r.get::<_,String>("work_title"),"round_index":r.get::<_,i32>("round_index"),
            "contract_matches_current":r.get::<_,bool>("contract_matches_current"),
            "requested_reviewer":r.get::<_,Value>("requested_reviewer"),
            "opened_at_unix_ms":r.get::<_,i64>("opened_at_unix_ms"),"wait_ms":r.get::<_,i64>("wait_ms")
        })).collect();
    }
    if !c.key.is_empty() || c.index < -1 || offset > total {
        return Err(PgError::CursorExpired);
    }
    let more = offset + (items.len() as i64) < total;
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
    let mut data = json!({"work_id":q.work_id,"workstream_id":stream,"recipient_person_id":q.recipient_person_id,"items":items,"total":total,"next_cursor":next,"truncated":more,
        "count_scope":"authorized_matches","execution_authorized":false,
        "as_of":{"at_unix_ms":stamp.get::<_,i64>(2),"source_snapshot_id":auth.snapshot,
            "project_revision":auth.revision.to_string(),"queue_revision":revision}});
    if q.op == "review.queue" {
        data["groups"] = json!(groups);
    }
    let response = json!({"protocol_version":1,"scope_id":"main","selection_basis":"project",
        "coordinator_epoch":auth.epoch,"project_status":auth.project_status,"source_snapshot_id":auth.snapshot,
        "project_revision":auth.revision.to_string(),"data":data});
    if serde_json::to_vec(&response)
        .map_err(|_| PgError::SourceDivergence)?
        .len()
        > q.max_context_bytes.unwrap_or(262144)
    {
        return Err(PgError::ResponseTooLarge);
    }
    Ok(response)
}
