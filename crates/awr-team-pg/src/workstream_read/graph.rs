//! Bounded graph pages for progress views, using the authenticated read snapshot.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

pub(super) async fn read(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    stream: &str,
    q: &WorkstreamQuery,
    binding: &str,
    cursor: &Cursor,
    limit: i64,
) -> PgResult<Value> {
    if q.cursor.is_some() && cursor.revision != auth.revision.to_string() {
        return Err(PgError::CursorExpired);
    }
    let observed: i64 = tx
        .query_one(
            "SELECT (extract(epoch FROM clock_timestamp())*1000)::bigint",
            &[],
        )
        .await?
        .get(0);
    let total: i64 = tx
        .query_one(
            "SELECT count(*) FROM awr_team.work_contracts c
         JOIN awr_team.workstream_snapshot_ownership o
           USING(tenant_id,project_id,snapshot_id,scope_id,work_id)
         WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.snapshot_id=$3 AND o.workstream_id=$4",
            &[&tenant, &project, &auth.snapshot, &stream],
        )
        .await?
        .get(0);
    let rows = tx
        .query(
            "SELECT c.work_id,c.title,c.contract_json,c.contract_hash,c.definition_state,
           r.state,p.display_name,
           (SELECT (extract(epoch FROM max(e.created_at))*1000)::bigint FROM awr_team.events e
             WHERE e.tenant_id=c.tenant_id AND e.project_id=c.project_id AND e.work_id=c.work_id)
         FROM awr_team.work_contracts c
         JOIN awr_team.workstream_snapshot_ownership o
           USING(tenant_id,project_id,snapshot_id,scope_id,work_id)
         LEFT JOIN awr_team.work_runtime r USING(tenant_id,project_id,scope_id,work_id)
         LEFT JOIN awr_team.task_responsibilities t
           ON t.tenant_id=c.tenant_id AND t.project_id=c.project_id AND t.work_id=c.work_id
         LEFT JOIN awr_team.persons p
           ON p.tenant_id=t.tenant_id AND p.project_id=t.project_id AND p.id=t.owner_person_id
         WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.snapshot_id=$3
           AND o.workstream_id=$4 AND c.work_id>$5 ORDER BY c.work_id LIMIT $6",
            &[
                &tenant,
                &project,
                &auth.snapshot,
                &stream,
                &cursor.key,
                &(limit + 1),
            ],
        )
        .await?;
    let mut contracts = Vec::new();
    let mut dependencies = BTreeSet::new();
    for row in rows.iter().take(limit as usize) {
        let contract: WorkContract =
            serde_json::from_value(row.get(2)).map_err(|_| PgError::SourceDivergence)?;
        let id: String = row.get(0);
        if contract.work_id.as_str() != id
            || contract.hash().map_err(|_| PgError::SourceDivergence)? != row.get::<_, String>(3)
        {
            return Err(PgError::SourceDivergence);
        }
        dependencies.extend(contract.required_dependencies.iter().cloned());
        contracts.push(contract);
    }
    // The caller's grants have already been narrowed by its live delegation.
    // References to an unreadable upstream never become graph nodes or labels.
    let readable_streams: Vec<String> = auth
        .catalog
        .workstreams
        .iter()
        .filter(|s| {
            auth.access
                .authorize(&auth.catalog, s.id, WorkstreamAction::Read)
                .is_ok()
        })
        .map(|s| s.id.to_string())
        .collect();
    let dependency_ids: Vec<_> = dependencies.into_iter().collect();
    let visible: BTreeSet<String> = tx
        .query(
            "SELECT work_id FROM awr_team.workstream_snapshot_ownership
         WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3
           AND workstream_id=ANY($4) AND work_id=ANY($5)",
            &[
                &tenant,
                &project,
                &auth.snapshot,
                &readable_streams,
                &dependency_ids,
            ],
        )
        .await?
        .into_iter()
        .map(|r| r.get(0))
        .collect();
    let work_ids: Vec<String> = rows.iter().take(limit as usize).map(|r| r.get(0)).collect();
    let claims = tx
        .query(
            "SELECT cl.work_id,cl.id,cl.actor_id,a.display_name,cl.session_id,
           (extract(epoch FROM cl.expires_at)*1000)::bigint,
           (extract(epoch FROM cl.acquired_at)*1000)::bigint
         FROM awr_team.claims cl JOIN awr_team.sessions s
           ON s.tenant_id=cl.tenant_id AND s.project_id=cl.project_id AND s.id=cl.session_id
         JOIN awr_team.workstream_snapshot_ownership o
           ON o.tenant_id=cl.tenant_id AND o.project_id=cl.project_id AND o.snapshot_id=$3
             AND o.work_id=cl.work_id AND o.workstream_id=cl.workstream_id
             AND o.ownership_version=cl.ownership_version
         JOIN awr_team.work_runtime r
           ON r.tenant_id=cl.tenant_id AND r.project_id=cl.project_id AND r.work_id=cl.work_id
             AND r.scope_id=cl.scope_id AND r.last_fence=cl.fence
         LEFT JOIN awr_team.actors a ON a.tenant_id=cl.tenant_id AND a.id=cl.actor_id
         WHERE cl.tenant_id=$1 AND cl.project_id=$2 AND cl.workstream_id=$4
           AND cl.work_id=ANY($5) AND cl.scope_id='main' AND cl.state='active'
           AND cl.coordinator_epoch=$6 AND cl.expires_at>to_timestamp($7::double precision/1000)
           AND s.state='active' AND s.work_id=cl.work_id AND s.workstream_id=cl.workstream_id
           AND s.ownership_version=cl.ownership_version ORDER BY cl.work_id,cl.id",
            &[
                &tenant,
                &project,
                &auth.snapshot,
                &stream,
                &work_ids,
                &auth.epoch,
                &(observed as f64),
            ],
        )
        .await?;
    let mut claims_by_work: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for claim in claims {
        claims_by_work.entry(claim.get(0)).or_default().push(json!({
            "id":claim.get::<_,String>(1), "actor_id":claim.get::<_,String>(2),
            "actor_name":claim.get::<_,Option<String>>(3), "session_id":claim.get::<_,String>(4),
            "expires_at_unix_ms":claim.get::<_,i64>(5),
            "acquired_at_unix_ms":claim.get::<_,i64>(6),
        }));
    }
    let items: Vec<_> = rows.iter().zip(contracts).map(|(row, contract)| {
        let id: String = row.get(0);
        let definition: String = row.get(4);
        let state = row.get::<_,Option<String>>(5).unwrap_or_else(|| match definition.as_str() {
            "draft" => "draft".into(),
            "cancelled" | "archived" => "cancelled".into(),
            _ => "planned".into(),
        });
        let dependencies: Vec<_> = contract.required_dependencies.iter()
            .filter(|id| visible.contains(*id)).cloned().collect();
        json!({"work_id":id, "external_key":contract.external_key, "title":row.get::<_,String>(1),
            "contract_hash":row.get::<_,String>(3), "definition_state":definition, "state":state,
            "dependencies_complete":dependencies.len()==contract.required_dependencies.len(),
            "required_dependencies":dependencies, "goals":contract.goals,
            "owner":row.get::<_,Option<String>>(6), "last_event_at":row.get::<_,Option<i64>>(7),
            "claims":claims_by_work.remove(&id).unwrap_or_default()})
    }).collect();
    let next = if rows.len() > limit as usize {
        next_cursor(
            binding,
            items.last().unwrap()["work_id"].as_str().unwrap(),
            auth.revision,
            -1,
        )
    } else {
        Value::Null
    };
    Ok(json!({"items":items,"total":total,"next_cursor":next,
        "queried_at_unix_ms":observed,"execution_authorized":false}))
}
