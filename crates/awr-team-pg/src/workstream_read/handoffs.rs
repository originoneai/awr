//! Authorized handoff metadata, without packages, consumption or receiving effects.
use super::*;

const RECORDS: &str = "WITH accessible AS (
    SELECT h.id,h.work_id,h.kind,h.status,h.version,h.from_person_id,h.to_person_id,
      h.created_at_ms,h.updated_at_ms,h.expires_at_ms,c.title AS work_title,
      fp.display_name AS from_display_name,tp.display_name AS to_display_name,
      (h.status IN ('proposed','inspected') AND (h.expires_at_ms IS NULL OR
        h.expires_at_ms>(extract(epoch FROM transaction_timestamp())*1000)::bigint)) AS pending,
      (h.status='timed_out' OR (h.status IN ('proposed','inspected') AND h.expires_at_ms IS NOT NULL
        AND h.expires_at_ms<=(extract(epoch FROM transaction_timestamp())*1000)::bigint)) AS expired
    FROM awr_team.team_handoffs h
    JOIN awr_team.work_contracts c ON c.tenant_id=h.tenant_id AND c.project_id=h.project_id
      AND c.snapshot_id=$3 AND c.scope_id='main' AND c.work_id=h.work_id
    JOIN awr_team.workstream_snapshot_ownership o
      ON o.tenant_id=c.tenant_id AND o.project_id=c.project_id AND o.snapshot_id=c.snapshot_id
        AND o.scope_id=c.scope_id AND o.work_id=c.work_id
    LEFT JOIN awr_team.persons fp ON fp.tenant_id=h.tenant_id AND fp.project_id=h.project_id AND fp.id=h.from_person_id
    LEFT JOIN awr_team.persons tp ON tp.tenant_id=h.tenant_id AND tp.project_id=h.project_id AND tp.id=h.to_person_id
    WHERE h.tenant_id=$1 AND h.project_id=$2
      AND (o.workstream_id=ANY($4) OR EXISTS (
        SELECT 1 FROM unnest($5::text[],$6::text[]) allowed(work_id,workstream_id)
        WHERE allowed.work_id=h.work_id AND allowed.workstream_id=o.workstream_id))
      AND ($7::text IS NULL OR h.work_id=$7) AND ($8::text IS NULL OR o.workstream_id=$8)
      AND ($9::text IS NULL OR h.to_person_id=$9)
), selected AS (
    SELECT * FROM accessible WHERE $10='all' OR ($10='pending' AND pending) OR ($10='history' AND NOT pending)
), records AS (
    SELECT *,row_number() OVER (ORDER BY created_at_ms,id) AS position FROM selected
)";

pub(super) async fn read(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    q: &WorkstreamQuery,
) -> PgResult<Value> {
    let bound = if let Some(work) = q.work_id.as_deref() {
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
    let (streams, task_ids, task_streams) = navigation::visible_work_scope(auth);
    let stream = bound.map(|id| id.to_string());
    if let Some(id) = bound {
        auth.access
            .authorize(&auth.catalog, id, WorkstreamAction::Read)
            .map_err(|_| PgError::Forbidden)?;
    }
    let state = q.handoff_state.as_deref().unwrap_or("pending");
    let params: &[&(dyn tokio_postgres::types::ToSql + Sync)] = &[
        &tenant,
        &project,
        &auth.snapshot,
        &streams,
        &task_ids,
        &task_streams,
        &q.work_id,
        &stream,
        &q.recipient_person_id,
        &state,
    ];
    let stamp = tx.query_one(&format!("{RECORDS}
      SELECT count(*),md5(COALESCE(string_agg(jsonb_build_array(id,work_id,kind,status,version,
        from_person_id,to_person_id,created_at_ms,updated_at_ms,expires_at_ms,work_title,
        from_display_name,to_display_name,pending,expired)::text,E'\\n' ORDER BY created_at_ms,id),'')),
        (extract(epoch FROM transaction_timestamp())*1000)::bigint FROM records"),params).await?;
    let revision: String = stamp.get(1);
    let binding = hash(
        &json!({"domain":"awr-handoff-list-v1","reader":auth.binding,
        "snapshot":auth.snapshot,"epoch":auth.epoch,"catalog":auth.catalog,"grants":auth.grant_versions,
        "work":q.work_id,"stream":stream,"recipient":q.recipient_person_id,"state":state,"revision":revision}),
    )?;
    let cursor = cursor(q, &binding)?;
    let total: i64 = stamp.get(0);
    let offset = i64::from(cursor.index) + 1;
    if !cursor.key.is_empty() || cursor.index < -1 || offset > total {
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
            &format!(
                "{RECORDS} SELECT * FROM records ORDER BY created_at_ms,id OFFSET $11 LIMIT $12"
            ),
            &page_params,
        )
        .await?;
    let more = rows.len() > limit as usize;
    let items: Vec<_> = rows.iter().take(limit as usize).map(|r| {
        let pending: bool = r.get("pending");
        let expired: bool = r.get("expired");
        let status: String = r.get("status");
        json!({"handoff_id":r.get::<_,String>("id"),"handoff_position":r.get::<_,i64>("position"),
            "work_id":r.get::<_,String>("work_id"),"work_title":r.get::<_,String>("work_title"),
            "kind":r.get::<_,String>("kind"),"status":status,"pending":pending,"expired":expired,
            "group":if pending{"pending"}else{"history"},
            "effective_status":if expired{"expired"}else{status.as_str()},
            "version":r.get::<_,i64>("version").to_string(),
            "from_person_id":r.get::<_,String>("from_person_id"),"to_person_id":r.get::<_,String>("to_person_id"),
            "from_display_name":r.get::<_,Option<String>>("from_display_name"),
            "to_display_name":r.get::<_,Option<String>>("to_display_name"),
            "created_at_unix_ms":r.get::<_,i64>("created_at_ms"),
            "updated_at_unix_ms":r.get::<_,i64>("updated_at_ms"),
            "expires_at_unix_ms":r.get::<_,Option<i64>>("expires_at_ms")})
    }).collect();
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
    let mut response = json!({"protocol_version":1,"scope_id":"main","coordinator_epoch":auth.epoch,
        "project_status":auth.project_status,"source_snapshot_id":auth.snapshot,"project_revision":auth.revision.to_string(),
        "data":{"work_id":q.work_id,"recipient_person_id":q.recipient_person_id,"handoff_state":state,
            "items":items,"total":total,"next_cursor":next,"truncated":more,
            "count_scope":"authorized_matches","count_limited":true,"hidden_count":null,
            "execution_authorized":false,
            "as_of":{"at_unix_ms":stamp.get::<_,i64>(2),"source_snapshot_id":auth.snapshot,
                "project_revision":auth.revision.to_string(),"handoffs_revision":revision}}});
    if let Some(id) = bound {
        let selected = auth
            .catalog
            .workstreams
            .iter()
            .find(|s| s.id == id)
            .ok_or(PgError::Forbidden)?;
        response["workstream_id"] = json!(id.to_string());
        response["authority_version"] = json!(selected.authority_version.to_string());
    }
    if serde_json::to_vec(&response)
        .map_err(|_| PgError::SourceDivergence)?
        .len()
        > q.max_context_bytes.unwrap_or(262144)
    {
        return Err(PgError::ResponseTooLarge);
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handoff_filters_are_bounded_and_cannot_be_used_with_other_queries() {
        for state in ["pending", "history", "all"] {
            let q: WorkstreamQuery = serde_json::from_value(json!({"protocol_version":1,"op":"handoff.list",
                "recipient_person_id":"recipient","handoff_state":state,"limit":20,"max_context_bytes":65536})).unwrap();
            q.validate().unwrap();
        }
        for value in [
            json!({"protocol_version":1,"op":"handoff.list","recipient_person_id":""}),
            json!({"protocol_version":1,"op":"handoff.list","handoff_state":"accepted"}),
            json!({"protocol_version":1,"op":"handoff.list","session_id":"session"}),
            json!({"protocol_version":1,"op":"work.next","recipient_person_id":"recipient"}),
            json!({"protocol_version":1,"op":"work.next","handoff_state":"all"}),
        ] {
            assert!(
                serde_json::from_value::<WorkstreamQuery>(value)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
    }
}
