//! Task-scoped assignment targets, never an administrative member directory.
use super::*;

const SCAN_WINDOW: i64 = 100;

pub(super) async fn read(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    q: &WorkstreamQuery,
    stream: Id,
    authority: u64,
    ownership: i64,
) -> PgResult<Value> {
    let work = q.work_id.as_deref().ok_or(PgError::Forbidden)?;
    let scoped = crate::delegation_auth::assignment_read_authority(auth, stream, work)?;
    let contract: String = tx
        .query_one(
            "SELECT contract_hash FROM awr_team.work_contracts
        WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND scope_id='main' AND work_id=$4",
            &[&tenant, &project, &auth.snapshot, &work],
        )
        .await?
        .get(0);
    let binding = hash(
        &json!({"domain":"awr-task-assignees-v1","identity":scoped.binding,
        "delegation":scoped.delegation_id,"actions":scoped.delegated_actions,
        "stream":stream,"authority":authority,"grant":auth.grant_versions[&stream],
        "snapshot":auth.snapshot,"epoch":auth.epoch,"ownership":ownership,
        "contract":contract,"revision":auth.revision,"work":work,"search":q.search}),
    )?;
    let c = cursor(q, &binding)?;
    if !c.key.is_empty()
        || c.index < -1
        || q.cursor.is_some() && c.revision != auth.revision.to_string()
    {
        return Err(PgError::CursorExpired);
    }
    let offset = i64::from(c.index) + 1;
    // Ordinals avoid exposing identifiers of ineligible members in the cursor.
    // Literal substring search does not interpret SQL wildcard characters.
    let rows = tx
        .query(
            "SELECT id,display_name FROM awr_team.persons
        WHERE tenant_id=$1 AND project_id=$2 AND status='active'
          AND ($3::text IS NULL OR strpos(lower(display_name),lower($3))>0)
        ORDER BY id OFFSET $4 LIMIT $5",
            &[
                &tenant,
                &project,
                &q.search.as_deref(),
                &offset,
                &SCAN_WINDOW,
            ],
        )
        .await?;
    let limit = usize::from(q.limit.unwrap_or(25));
    let mut items = Vec::new();
    let mut scanned = 0;
    for row in &rows {
        let target: String = row.get(0);
        scanned += 1;
        match crate::workstream_command::task_intake::eligible_target(
            tx,
            tenant,
            project,
            stream,
            i64::try_from(authority).map_err(|_| PgError::SourceDivergence)?,
            &target,
        )
        .await
        {
            Ok(_) => items.push(json!({"assignee_person_id":target,"name":row.get::<_,String>(1)})),
            Err(PgError::Forbidden) => {}
            Err(error) => return Err(error),
        }
        if items.len() == limit {
            break;
        }
    }
    let more = scanned < rows.len() || rows.len() == SCAN_WINDOW as usize;
    let next = if more {
        let index =
            i32::try_from(offset + scanned as i64 - 1).map_err(|_| PgError::CursorExpired)?;
        next_cursor(&binding, "", auth.revision, index)
    } else {
        Value::Null
    };
    Ok(json!({"work_id":work,"items":items,"next_cursor":next,
        "advisory":true,"assignment_rechecks_eligibility":true,
        "next_step":"Follow next_cursor even for an empty page; refresh work.prepare before task.assign."}))
}
