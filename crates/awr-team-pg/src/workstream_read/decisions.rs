//! Scoped ordinary questions and their recorded answers, including session-independent continuation.
use super::*;
use crate::workstream_command::decisions::{basis, record};

pub(super) fn guidance(data: &Value, previous: Value) -> Value {
    if !matches!(
        previous["code"].as_str(),
        Some("wait_for_change" | "blocked")
    ) {
        return previous;
    }
    let Some(question) = data["ordinary_decisions"]["items"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["state"] == "open"))
    else {
        return previous;
    };
    json!({"code":"inspect_ordinary_decision","when":"a persisted ordinary question is open",
        "because":["the selected question has no confirmed reply"],
        "action":{"op":"decisions.list","query":{"protocol_version":1,"op":"decisions.list",
            "work_id":data["work_id"],"decision_id":question["decision_id"]},
            "note":"Read this question. A selected authorized member may use decision.respond from an own active session. Withdraw and reissue a changed baseline."},
        "recheck_on":"reply, withdrawal, baseline or permission change"})
}

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
        Some(binding.workstream_id.to_string())
    } else {
        q.workstream_id.map(|id| id.to_string())
    };
    let (streams, tasks, task_streams) = navigation::visible_work_scope(auth);
    let binding = hash(
        &json!({"domain":"awr-ordinary-decisions-v1","reader":auth.binding,
        "snapshot":auth.snapshot,"epoch":auth.epoch,"catalog":auth.catalog,"grants":auth.grant_versions,
        "work":q.work_id,"stream":bound,"decision":q.decision_id,"state":q.decision_state}),
    )?;
    let cursor = cursor(q, &binding)?;
    if cursor.index != -1 {
        return Err(PgError::CursorExpired);
    }
    let limit = i64::from(q.limit.unwrap_or(20));
    let state = q.decision_state.as_deref().unwrap_or("open");
    let rows = tx.query("SELECT w.id,w.work_id,c.title FROM awr_team.wait_items w
        JOIN awr_team.workstream_snapshot_ownership o ON o.tenant_id=w.tenant_id AND o.project_id=w.project_id
          AND o.snapshot_id=$3 AND o.scope_id='main' AND o.work_id=w.work_id
        JOIN awr_team.work_contracts c ON c.tenant_id=w.tenant_id AND c.project_id=w.project_id
          AND c.snapshot_id=o.snapshot_id AND c.scope_id=o.scope_id AND c.work_id=w.work_id
        WHERE w.tenant_id=$1 AND w.project_id=$2 AND w.decision_key IS NOT NULL
          AND (o.workstream_id=ANY($4) OR EXISTS(SELECT 1 FROM unnest($5::text[],$6::text[]) allowed(work_id,workstream_id)
            WHERE allowed.work_id=w.work_id AND allowed.workstream_id=o.workstream_id))
          AND ($7::text IS NULL OR w.work_id=$7) AND ($8::text IS NULL OR o.workstream_id=$8)
          AND ($9::text IS NULL OR w.id=$9) AND ($10='all' OR w.state=$10)
          AND ($11='' OR w.id<$11) ORDER BY w.id DESC LIMIT $12",
        &[&tenant,&project,&auth.snapshot,&streams,&tasks,&task_streams,&q.work_id,&bound,
          &q.decision_id,&state,&cursor.key,&(limit+1)]).await?;
    let more = rows.len() > limit as usize;
    let mut items = Vec::new();
    for row in rows.iter().take(limit as usize) {
        let id: String = row.get(0);
        let work: String = row.get(1);
        let current = basis(tx, tenant, project, auth, &work).await?;
        let mut item = record(tx, tenant, project, &work, &id, &current).await?;
        item["work_title"] = json!(row.get::<_, String>(2));
        items.push(item);
    }
    let next = if more {
        next_cursor(
            &binding,
            items.last().unwrap()["decision_id"].as_str().unwrap(),
            0,
            -1,
        )
    } else {
        Value::Null
    };
    let at: i64 = tx
        .query_one(
            "SELECT (extract(epoch FROM transaction_timestamp())*1000)::bigint",
            &[],
        )
        .await?
        .get(0);
    let mut result = json!({"protocol_version":1,"scope_id":"main","selection_basis":"project",
        "coordinator_epoch":auth.epoch,"project_status":auth.project_status,
        "source_snapshot_id":auth.snapshot,"project_revision":auth.revision.to_string(),
        "data":{"work_id":q.work_id,"decision_id":q.decision_id,"decision_state":state,
            "items":items,"next_cursor":next,"truncated":more,"execution_authorized":false,
            "as_of":{"at_unix_ms":at,"source_snapshot_id":auth.snapshot,"project_revision":auth.revision.to_string()}}});
    if let Some(stream) = bound.as_deref() {
        let stream = stream.parse().map_err(|_| PgError::SourceDivergence)?;
        result["workstream_id"] = json!(stream);
        result["authority_version"] =
            json!(auth.catalog.get(stream)?.authority_version.to_string());
        result["selection_basis"] = json!(if q.work_id.is_some() {
            "work"
        } else {
            "explicit"
        });
    }
    if serde_json::to_vec(&result)
        .map_err(|_| PgError::SourceDivergence)?
        .len()
        > q.max_context_bytes.unwrap_or(262144)
    {
        return Err(PgError::ResponseTooLarge);
    }
    Ok(result)
}

/// Stable selected facts enter consumed context. Read timestamps and audit cursors stay outside it.
pub(super) async fn context(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    work: &str,
) -> PgResult<Value> {
    let rows = tx
        .query(
            "SELECT id FROM awr_team.wait_items
        WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND decision_key IS NOT NULL
        ORDER BY (state='open') DESC,id DESC LIMIT 4",
            &[&tenant, &project, &work],
        )
        .await?;
    if rows.is_empty() {
        return Ok(json!({"items":[],"truncated":false}));
    }
    let current = basis(tx, tenant, project, auth, work).await?;
    let mut items = Vec::new();
    for row in rows.iter().take(3) {
        items.push(
            record(
                tx,
                tenant,
                project,
                work,
                &row.get::<_, String>(0),
                &current,
            )
            .await?,
        );
    }
    Ok(json!({"items":items,"truncated":rows.len()>3,
        "next_query":{"protocol_version":1,"op":"decisions.list","work_id":work,"decision_state":"all"}}))
}
