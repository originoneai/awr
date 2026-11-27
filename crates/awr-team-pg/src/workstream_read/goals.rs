//! Deterministic goal coverage over registered source facts and task contracts.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

fn state_class(status: &str) -> &str {
    match status.trim().to_lowercase().as_str() {
        "active" | "confirmed" => "active",
        "cancelled" => "cancelled_goal",
        "archived" => "archived_goal",
        _ => "unknown",
    }
}

fn binding(
    auth: &ReaderAuthority,
    stream: Option<Id>,
    op: &str,
    facts: &Value,
) -> PgResult<String> {
    hash(&json!({"domain":"awr-goal-read-v1","reader":auth.binding,
        "snapshot":auth.snapshot,"revision":auth.revision,"epoch":auth.epoch,
        "catalog":auth.catalog,"grants":auth.grant_versions,"stream":stream,"op":op,"facts":facts}))
}

fn page(q: &WorkstreamQuery, binding: &str, len: usize) -> PgResult<(usize, usize, Value)> {
    let c = cursor(q, binding)?;
    let start = c
        .index
        .checked_add(1)
        .and_then(|n| usize::try_from(n).ok())
        .filter(|start| *start <= len)
        .ok_or(PgError::CursorExpired)?;
    if !c.key.is_empty() || c.revision != "0" {
        return Err(PgError::CursorExpired);
    }
    let end = start
        .saturating_add(usize::from(q.limit.unwrap_or(20)))
        .min(len);
    let next = if end < len {
        next_cursor(
            binding,
            "",
            0,
            i32::try_from(end - 1).map_err(|_| PgError::ResponseTooLarge)?,
        )
    } else {
        Value::Null
    };
    Ok((start, end, next))
}

pub(super) async fn read(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    q: &WorkstreamQuery,
) -> PgResult<Value> {
    if let Some(stream) = q.workstream_id {
        authorize_query(auth, Some(stream), None, &q.op)?;
        auth.access
            .authorize(&auth.catalog, stream, WorkstreamAction::Read)
            .map_err(|_| PgError::Forbidden)?;
    }
    let (streams, tasks, task_streams) = navigation::visible_work_scope(auth);
    let full_project = q.workstream_id.is_none()
        && auth
            .catalog
            .workstreams
            .iter()
            .all(|stream| streams.contains(&stream.id.to_string()));
    let mut eligible: BTreeSet<String> = auth
        .catalog
        .workstreams
        .iter()
        .filter(|stream| {
            streams.contains(&stream.id.to_string())
                && q.workstream_id.is_none_or(|id| id == stream.id)
        })
        .flat_map(|stream| stream.goal_keys.clone())
        .collect();
    let files = active_source_files(tx, tenant, project).await?;
    let goals = crate::source::registered_goal_ledger(&files)?;
    let by_key: BTreeMap<_, _> = goals
        .iter()
        .map(|goal| (goal.meta.external_key.as_str(), goal))
        .collect();
    // Hidden rows contribute only counts/coverage. Their keys, titles and status
    // are never included in a public record or a continuation cursor.
    let rows = tx.query("SELECT c.work_id,c.title,c.contract_json,c.definition_state,
        COALESCE(r.state,CASE WHEN c.definition_state='draft' THEN 'draft' ELSE 'planned' END),(o.workstream_id=ANY($4) OR EXISTS(
          SELECT 1 FROM unnest($5::text[],$6::text[]) allowed(work_id,workstream_id)
          WHERE allowed.work_id=c.work_id AND allowed.workstream_id=o.workstream_id))
        FROM awr_team.work_contracts c
        JOIN awr_team.workstream_snapshot_ownership o USING(tenant_id,project_id,snapshot_id,scope_id,work_id)
        LEFT JOIN awr_team.work_runtime r USING(tenant_id,project_id,scope_id,work_id)
        WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.snapshot_id=$3 AND c.scope_id='main'
          AND ($7::text IS NULL OR o.workstream_id=$7) ORDER BY c.work_id",
        &[&tenant,&project,&auth.snapshot,&streams,&tasks,&task_streams,
          &q.workstream_id.map(|id| id.to_string())]).await?;
    let mut references = BTreeMap::<String, Vec<Value>>::new();
    let mut hidden = BTreeMap::<String, usize>::new();
    let mut covered = BTreeSet::new();
    let mut orphans = Vec::new();
    for row in rows {
        let definition: String = row.get(3);
        let runtime: String = row.get(4);
        if definition == "archived" || runtime == "archived" {
            continue;
        }
        let status = if definition == "cancelled" {
            "cancelled"
        } else {
            &runtime
        };
        let contract: WorkContract =
            serde_json::from_value(row.get(2)).map_err(|_| PgError::SourceDivergence)?;
        let visible: bool = row.get(5);
        for goal in contract.goals.iter().collect::<BTreeSet<_>>() {
            if status != "cancelled" {
                covered.insert(goal.clone());
            }
            if !visible {
                *hidden.entry(goal.clone()).or_default() += 1;
                continue;
            }
            eligible.insert(goal.clone());
            let task = json!({"work_id":row.get::<_,String>(0),"key":contract.external_key,
                "title":row.get::<_,String>(1),"status":status});
            references
                .entry(goal.clone())
                .or_default()
                .push(task.clone());
            let reason = by_key
                .get(goal.as_str())
                .map_or("unknown_goal", |g| state_class(&g.status));
            if status != "cancelled"
                && matches!(reason, "unknown_goal" | "cancelled_goal" | "archived_goal")
            {
                orphans
                    .push(json!({"kind":"orphan_work","goal_id":goal,"task":task,"reason":reason}));
            }
        }
    }
    let mut records = Vec::new();
    for (key, goal) in by_key {
        if !full_project && !eligible.contains(key) {
            continue;
        }
        let tasks = references.remove(key).unwrap_or_default();
        let hidden_count = hidden.get(key).copied().unwrap_or_default();
        records.push(
            json!({"kind":"goal","goal_id":key,"key":key,"title":goal.title,
            "status":goal.status,"success_criteria":goal.success_criteria,
            "departments":[],"department_reason":"goal_department_not_registered",
            "tasks":tasks,"hidden_task_count":hidden_count,
            "uncovered":state_class(&goal.status)=="active" && !covered.contains(key)}),
        );
    }
    let mut data = if q.op == "goal.inspect" {
        let mut goal = records
            .into_iter()
            .find(|goal| goal["goal_id"].as_str() == q.goal_id.as_deref())
            .ok_or(PgError::Forbidden)?;
        let binding = binding(auth, q.workstream_id, "goal.inspect", &goal)?;
        let tasks = goal["tasks"].as_array().ok_or(PgError::SourceDivergence)?;
        let (start, end, next) = page(q, &binding, tasks.len())?;
        let total = tasks.len();
        goal["tasks"] = json!(&tasks[start..end]);
        json!({"goal":goal,"total":total,"next_cursor":next,"truncated":end<total,
            "orphan_work":orphans.into_iter().filter(|o| o["goal_id"].as_str()==q.goal_id.as_deref())
                .filter(|o| goal["tasks"].as_array().unwrap().iter().any(|task| *task==o["task"]))
                .collect::<Vec<_>>()})
    } else {
        // Each goal includes a bounded task preview. Its continuation is an
        // exact goal.inspect query, independent of the outer collection page.
        for goal in &mut records {
            let task_binding = binding(auth, q.workstream_id, "goal.inspect", goal)?;
            let tasks = goal["tasks"].as_array().unwrap();
            let total = tasks.len();
            let next = if total > 20 {
                next_cursor(&task_binding, "", 0, 19)
            } else {
                Value::Null
            };
            goal["tasks"] = json!(&tasks[..total.min(20)]);
            goal["task_total"] = json!(total);
            goal["tasks_next_cursor"] = next;
        }
        records.extend(orphans);
        let binding = binding(auth, q.workstream_id, "goals.list", &json!(records))?;
        let (start, end, next) = page(q, &binding, records.len())?;
        json!({"items":&records[start..end],"total":records.len(),"next_cursor":next,"truncated":end<records.len()})
    };
    let at: i64 = tx
        .query_one(
            "SELECT (extract(epoch FROM transaction_timestamp())*1000)::bigint",
            &[],
        )
        .await?
        .get(0);
    data["read_only"] = json!(true);
    data["execution_authorized"] = json!(false);
    data["as_of"] = json!({"at_unix_ms":at,"source_snapshot_id":auth.snapshot,"project_revision":auth.revision.to_string()});
    let mut result = json!({"protocol_version":1,"scope_id":"main","selection_basis":"project",
        "source_snapshot_id":auth.snapshot,"coordinator_epoch":auth.epoch,"project_revision":auth.revision.to_string(),
        "project_status":auth.project_status,"data":data});
    if let Some(stream) = q.workstream_id {
        result["workstream_id"] = json!(stream);
        result["authority_version"] =
            json!(auth.catalog.get(stream)?.authority_version.to_string());
        result["selection_basis"] = json!("explicit");
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
