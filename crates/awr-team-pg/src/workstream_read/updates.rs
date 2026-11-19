//! Read-only project period updates, ordered by period with keyset pagination.
use super::*;

const RECORD_SELECT: &str =
    "SELECT u.id,u.period_kind,u.period_key,u.starts_on::text,u.ends_on::text,
    u.status,u.summary,u.author_person_id,u.author_display_name,
    (extract(epoch FROM u.recorded_at)*1000)::bigint,u.version,u.supersedes_update_id,
    u.fact_snapshot_ref";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateCursor {
    binding: String,
    starts_on: String,
    ends_on: String,
    period_key: String,
    update_id: String,
}

fn binding(auth: &ReaderAuthority) -> PgResult<String> {
    hash(
        &json!({"domain":"awr-project-period-updates-v1","reader":auth.binding,
        "snapshot":auth.snapshot,"epoch":auth.epoch,"catalog":auth.catalog,
        "grants":auth.grant_versions}),
    )
}

fn decode_cursor(q: &WorkstreamQuery, binding: &str) -> PgResult<Option<UpdateCursor>> {
    let Some(raw) = q.cursor.as_deref() else {
        return Ok(None);
    };
    let cursor = serde_json::from_str::<UpdateCursor>(raw).map_err(|_| PgError::CursorExpired)?;
    if cursor.binding != binding
        || !crate::workstream_command::valid_date(&cursor.starts_on)
        || !crate::workstream_command::valid_date(&cursor.ends_on)
        || cursor.period_key.is_empty()
        || cursor.update_id.is_empty()
    {
        return Err(PgError::CursorExpired);
    }
    Ok(Some(cursor))
}

fn next_cursor(binding: &str, row: &tokio_postgres::Row) -> PgResult<Value> {
    serde_json::to_string(&UpdateCursor {
        binding: binding.into(),
        starts_on: row.get(3),
        ends_on: row.get(4),
        period_key: row.get(2),
        update_id: row.get(0),
    })
    .map(Value::String)
    .map_err(|_| PgError::SourceDivergence)
}

fn record(row: &tokio_postgres::Row, current: bool) -> Value {
    json!({
        "update_id":row.get::<_,String>(0),"period_kind":row.get::<_,String>(1),
        "period_key":row.get::<_,String>(2),"starts_on":row.get::<_,String>(3),
        "ends_on":row.get::<_,String>(4),"status":row.get::<_,String>(5),
        "summary":row.get::<_,String>(6),
        "author":{"person_id":row.get::<_,String>(7),"display_name":row.get::<_,String>(8)},
        "recorded_at_unix_ms":row.get::<_,i64>(9),"version":row.get::<_,i64>(10).to_string(),
        "supersedes_update_id":row.get::<_,Option<String>>(11),
        "fact_snapshot_ref":row.get::<_,Option<String>>(12),"current":current
    })
}

async fn as_of(tx: &Transaction<'_>, auth: &ReaderAuthority) -> PgResult<Value> {
    let at: i64 = tx
        .query_one(
            "SELECT (extract(epoch FROM transaction_timestamp())*1000)::bigint",
            &[],
        )
        .await?
        .get(0);
    Ok(json!({"at_unix_ms":at,"source_snapshot_id":auth.snapshot,
        "project_revision":auth.revision.to_string()}))
}

fn envelope(auth: &ReaderAuthority, data: Value) -> Value {
    json!({"protocol_version":1,"scope_id":"main","selection_basis":"project",
        "coordinator_epoch":auth.epoch,"project_status":auth.project_status,
        "source_snapshot_id":auth.snapshot,"project_revision":auth.revision.to_string(),
        "data":data})
}

pub(super) async fn read(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    q: &WorkstreamQuery,
) -> PgResult<Value> {
    match q.op.as_str() {
        "updates.list" => list(tx, tenant, project, auth, q).await,
        "update.inspect" => inspect(tx, tenant, project, auth, q).await,
        _ => Err(PgError::Unsupported("project update read".into())),
    }
}

async fn list(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    q: &WorkstreamQuery,
) -> PgResult<Value> {
    let binding = binding(auth)?;
    let cursor = decode_cursor(q, &binding)?;
    let limit = i64::from(q.limit.unwrap_or(20));
    let (starts, ends, key, id) = cursor.as_ref().map_or((None, None, None, None), |cursor| {
        (
            Some(cursor.starts_on.as_str()),
            Some(cursor.ends_on.as_str()),
            Some(cursor.period_key.as_str()),
            Some(cursor.update_id.as_str()),
        )
    });
    let sql = format!(
        "{RECORD_SELECT},TRUE FROM awr_team.project_period_update_current c
        JOIN awr_team.project_period_updates u ON u.tenant_id=c.tenant_id
          AND u.project_id=c.project_id AND u.id=c.update_id
        WHERE u.tenant_id=$1 AND u.project_id=$2
          AND ($3::boolean IS FALSE OR (u.starts_on,u.ends_on,u.period_key,u.id)
              < ($4::text::date,$5::text::date,$6::text,$7::text))
        ORDER BY u.starts_on DESC,u.ends_on DESC,u.period_key DESC,u.id DESC LIMIT $8"
    );
    let rows = tx
        .query(
            &sql,
            &[
                &tenant,
                &project,
                &cursor.is_some(),
                &starts,
                &ends,
                &key,
                &id,
                &(limit + 1),
            ],
        )
        .await?;
    let more = rows.len() > limit as usize;
    let visible = rows.iter().take(limit as usize).collect::<Vec<_>>();
    let items = visible
        .iter()
        .map(|row| record(row, true))
        .collect::<Vec<_>>();
    let next = if more {
        visible
            .last()
            .map(|row| next_cursor(&binding, row))
            .transpose()?
            .unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    let result = envelope(
        auth,
        json!({"project_id":project,"items":items,"next_cursor":next,"truncated":more,
            "as_of":as_of(tx,auth).await?,"execution_authorized":false}),
    );
    if serde_json::to_vec(&result)
        .map_err(|_| PgError::SourceDivergence)?
        .len()
        > q.max_context_bytes.unwrap_or(262144)
    {
        return Err(PgError::ResponseTooLarge);
    }
    Ok(result)
}

async fn visible_work_ids(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
) -> PgResult<std::collections::BTreeSet<String>> {
    let (streams, tasks, task_streams) = super::navigation::visible_work_scope(auth);
    let rows = tx
        .query(
            "SELECT o.work_id FROM awr_team.workstream_snapshot_ownership o
             WHERE o.tenant_id=$1 AND o.project_id=$2 AND o.snapshot_id=$3 AND o.scope_id='main'
               AND (o.workstream_id=ANY($4) OR EXISTS(
                   SELECT 1 FROM unnest($5::text[],$6::text[]) allowed(work_id,workstream_id)
                   WHERE allowed.work_id=o.work_id AND allowed.workstream_id=o.workstream_id))",
            &[
                &tenant,
                &project,
                &auth.snapshot,
                &streams,
                &tasks,
                &task_streams,
            ],
        )
        .await?;
    Ok(rows.into_iter().map(|row| row.get(0)).collect())
}

async fn inspect(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    q: &WorkstreamQuery,
) -> PgResult<Value> {
    let id = q.update_id.as_deref().ok_or(PgError::Forbidden)?;
    let sql = format!(
        "{RECORD_SELECT},EXISTS(
            SELECT 1 FROM awr_team.project_period_update_current c
            WHERE c.tenant_id=u.tenant_id AND c.project_id=u.project_id AND c.update_id=u.id),
            u.fact_snapshot_json
        FROM awr_team.project_period_updates u
        WHERE u.tenant_id=$1 AND u.project_id=$2 AND u.id=$3"
    );
    let row = tx
        .query_opt(&sql, &[&tenant, &project, &id])
        .await?
        .ok_or(PgError::Forbidden)?;
    let current: bool = row.get(13);
    let stored = row.get::<_, Option<Value>>(14);
    let (facts, coverage) = if let Some(stored) = stored {
        let visible = visible_work_ids(tx, tenant, project, auth).await?;
        let (facts, hidden) = crate::workstream_command::project_fact_snapshot(&stored, &visible)?;
        (
            facts,
            json!({"complete":hidden==0,"hidden_work_count":hidden}),
        )
    } else {
        (Value::Null, Value::Null)
    };
    Ok(envelope(
        auth,
        json!({"project_id":project,"update":record(&row,current),
            "fact_snapshot":facts,"fact_snapshot_coverage":coverage,
            "as_of":as_of(tx,auth).await?,"execution_authorized":false}),
    ))
}
