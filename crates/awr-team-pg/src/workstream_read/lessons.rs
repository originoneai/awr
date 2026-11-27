//! Scoped lesson records; reads grant neither confirmation nor execution authority.
use super::*;

pub(super) async fn read(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    q: &WorkstreamQuery,
) -> PgResult<Value> {
    let work = q.work_id.as_deref().ok_or(PgError::Forbidden)?;
    let (bound, _) = work_binding(tx, tenant, project, auth, work).await?;
    if q.workstream_id.is_some_and(|id| id != bound.workstream_id) {
        return Err(PgError::Forbidden);
    }
    let binding = hash(&json!({"domain":"awr-lessons-v1","reader":auth.binding,
        "snapshot":auth.snapshot,"epoch":auth.epoch,"grants":auth.grant_versions,
        "catalog":auth.catalog,"work":work}))?;
    let page = cursor(q, &binding)?;
    if page.index != -1 {
        return Err(PgError::CursorExpired);
    }
    let limit = i64::from(q.limit.unwrap_or(20));
    let mut items = crate::workstream_command::lessons::records(
        tx,
        tenant,
        project,
        auth,
        work,
        &page.key,
        limit + 1,
    )
    .await?;
    let more = items.len() > limit as usize;
    items.truncate(limit as usize);
    let next = if more {
        next_cursor(
            &binding,
            items.last().unwrap()["lesson_id"].as_str().unwrap(),
            0,
            -1,
        )
    } else {
        Value::Null
    };
    let result = json!({"protocol_version":1,"scope_id":"main","selection_basis":"work",
        "source_snapshot_id":auth.snapshot,"project_revision":auth.revision.to_string(),
        "coordinator_epoch":auth.epoch,"workstream_id":bound.workstream_id,
        "authority_version":auth.catalog.get(bound.workstream_id)?.authority_version.to_string(),
        "data":{"work_id":work,"items":items,"next_cursor":next,"truncated":more,
            "use_for_personnel_evaluation":false,"execution_authorized":false}});
    if serde_json::to_vec(&result)
        .map_err(|_| PgError::SourceDivergence)?
        .len()
        > q.max_context_bytes.unwrap_or(262144)
    {
        return Err(PgError::ResponseTooLarge);
    }
    Ok(result)
}

pub(super) async fn context(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    work: &str,
) -> PgResult<Value> {
    let mut items =
        crate::workstream_command::lessons::records(tx, tenant, project, auth, work, "", 4).await?;
    let more = items.len() > 3;
    items.truncate(3);
    Ok(json!({"items":items,"truncated":more,
        "next_query":{"protocol_version":1,"op":"lessons.list","work_id":work}}))
}
