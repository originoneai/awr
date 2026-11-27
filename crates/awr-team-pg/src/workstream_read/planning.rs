//! Read-only planning observations, filtered before counts and pagination.
use super::*;
use awr_team::DraftChange;
use std::collections::{BTreeMap, BTreeSet};

// A candidate is disclosed only when all its work belongs to readable scopes.
// Scope filtering also covers uncreated work, before it has an ownership row.
const FILTER: &str = "c.tenant_id=$1 AND c.project_id=$2 AND NOT EXISTS (
    SELECT 1 FROM jsonb_array_elements(c.changes_json) change WHERE NOT (
      EXISTS(SELECT 1 FROM awr_team.workstream_snapshot_ownership o
        WHERE o.tenant_id=c.tenant_id AND o.project_id=c.project_id AND o.snapshot_id=$3
          AND o.work_id=change#>>'{after,work_id}' AND o.workstream_id=ANY($4))
      OR (NOT EXISTS(SELECT 1 FROM awr_team.workstream_snapshot_ownership o
        WHERE o.tenant_id=c.tenant_id AND o.project_id=c.project_id AND o.snapshot_id=$3
          AND o.work_id=change#>>'{after,work_id}')
        AND ((change#>>'{after,workstream}')=ANY($5)
          OR (change#>>'{after,workstream}' IS NULL AND $6)))
    )) AND ($7::text IS NULL OR EXISTS (
      SELECT 1 FROM jsonb_array_elements(c.changes_json) change
      WHERE change#>>'{after,workstream}' IN ($7,$8)
        OR EXISTS(SELECT 1 FROM awr_team.workstream_snapshot_ownership o
          WHERE o.tenant_id=c.tenant_id AND o.project_id=c.project_id AND o.snapshot_id=$3
            AND o.work_id=change#>>'{after,work_id}' AND o.workstream_id=$7)))";

fn nullable(value: Value, reason: &str) -> Value {
    if value.is_null() {
        json!({"value":null,"reason":reason})
    } else {
        json!({"value":value,"reason":null})
    }
}

fn diff(
    change: &DraftChange,
    current: Option<&Value>,
    readable: &BTreeSet<String>,
) -> PgResult<Value> {
    let desired = serde_json::to_value(&change.after).map_err(|_| PgError::SourceDivergence)?;
    let mut fields = Vec::new();
    for field in [
        "title",
        "goals",
        "acceptance",
        "scope_paths",
        "required_dependencies",
        "dependency_acceptance",
        "execution_settlement",
        "definition_state",
        "completion_policy",
        "hard_rules",
        "verification_requirements",
        "examples",
    ] {
        let before = current
            .and_then(|c| c.get(field))
            .cloned()
            .unwrap_or(Value::Null);
        // Optional omitted source fields mean retention, not deletion of inherited rules.
        let after = desired
            .get(field)
            .filter(|v| !v.is_null())
            .cloned()
            .unwrap_or_else(|| before.clone());
        if before == after {
            continue;
        }
        let disclose = |value: Value| {
            let restricted = match field {
                "required_dependencies" => value.as_array().is_some_and(|ids| {
                    ids.iter()
                        .any(|id| id.as_str().is_none_or(|id| !readable.contains(id)))
                }),
                "dependency_acceptance" => value
                    .as_object()
                    .is_some_and(|modes| modes.keys().any(|id| !readable.contains(id))),
                _ => false,
            };
            if restricted {
                nullable(Value::Null, "permission_restricted")
            } else {
                nullable(value, "absent_from_current_plan")
            }
        };
        fields.push(json!({"field":field,"before":disclose(before),"after":disclose(after)}));
    }
    let kind = if fields.is_empty() {
        "unchanged"
    } else if current.is_none() {
        "added"
    } else if matches!(
        desired["definition_state"].as_str(),
        Some("cancelled" | "archived")
    ) {
        "removed"
    } else {
        "changed"
    };
    Ok(
        json!({"work_id":change.after.work_id,"title":change.after.title,
        "kind":kind,"fields":fields}),
    )
}

pub(super) async fn read(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    q: &WorkstreamQuery,
) -> PgResult<Value> {
    let visible: Vec<_> = auth
        .catalog
        .workstreams
        .iter()
        .filter(|s| {
            auth.access
                .authorize(&auth.catalog, s.id, WorkstreamAction::Read)
                .is_ok()
        })
        .collect();
    let streams: Vec<String> = visible.iter().map(|s| s.id.to_string()).collect();
    let keys: Vec<String> = visible
        .iter()
        .flat_map(|s| [s.id.to_string(), s.external_key.clone()])
        .collect();
    let active_streams = auth
        .catalog
        .workstreams
        .iter()
        .filter(|s| s.state == awr_core::WorkstreamState::Active)
        .count();
    let all_readable = active_streams > 0
        && visible
            .iter()
            .filter(|s| s.state == awr_core::WorkstreamState::Active)
            .count()
            == active_streams;
    let selected = if let Some(id) = q.workstream_id {
        Some(
            *visible
                .iter()
                .find(|s| s.id == id)
                .ok_or(PgError::Forbidden)?,
        )
    } else {
        None
    };
    let selected_id = selected.map(|s| s.id.to_string());
    let selected_key = selected.map(|s| s.external_key.clone());
    let params: &[&(dyn tokio_postgres::types::ToSql + Sync)] = &[
        &tenant,
        &project,
        &auth.snapshot,
        &streams,
        &keys,
        &all_readable,
        &selected_id,
        &selected_key,
    ];
    let stamp = tx
        .query_one(
            &format!(
                "SELECT count(*),md5(COALESCE(string_agg(
        c.id||c.candidate_digest||c.state||c.updated_at::text,'|' ORDER BY c.id),'')),
        (extract(epoch FROM statement_timestamp())*1000)::bigint,
        (SELECT authority_epoch::text FROM awr_team.projects WHERE tenant_id=$1 AND id=$2),
        (SELECT manifest_digest FROM awr_team.source_snapshots
          WHERE tenant_id=$1 AND project_id=$2 AND id=$3)
        FROM awr_team.planning_candidates c WHERE {FILTER}"
            ),
            params,
        )
        .await?;
    let planning_revision: String = stamp.get(1);
    let binding = hash(
        &json!({"auth":auth.binding,"snapshot":auth.snapshot,"epoch":auth.epoch,
        "op":q.op,"stream":selected_id,"planning_revision":planning_revision}),
    )?;
    let cursor = cursor(q, &binding)?;
    let limit = i64::from(q.limit.unwrap_or(20));
    let page_limit = limit + 1;
    let mut page_params = params.to_vec();
    page_params.extend([
        &cursor.key as &(dyn tokio_postgres::types::ToSql + Sync),
        &page_limit,
    ]);
    let rows=tx.query(&format!("SELECT c.id,c.author_actor_id,a.display_name,c.author_person_id,
        c.baseline_digest,c.baseline_epoch,c.candidate_digest,c.draft_revision,c.state,c.changes_json,
        (extract(epoch FROM c.created_at)*1000)::bigint,
        (extract(epoch FROM c.updated_at)*1000)::bigint,
        approval.body,publication.body,activation.body,rejection.body
        FROM awr_team.planning_candidates c
        LEFT JOIN awr_team.actors a ON a.tenant_id=c.tenant_id AND a.id=c.author_actor_id
        LEFT JOIN LATERAL (
          SELECT jsonb_build_object('actor_id',p.approver_actor_id,'display_name',a.display_name,
            'at_unix_ms',(extract(epoch FROM p.decided_at)*1000)::bigint,'self_approved',p.self_approved) body
          FROM awr_team.planning_approvals p LEFT JOIN awr_team.actors a ON a.tenant_id=p.tenant_id AND a.id=p.approver_actor_id
          WHERE p.tenant_id=c.tenant_id AND p.project_id=c.project_id AND p.candidate_id=c.id
            AND p.candidate_digest=c.candidate_digest AND p.draft_revision=c.draft_revision
          ORDER BY p.decided_at DESC,p.id DESC LIMIT 1) approval ON true
        LEFT JOIN LATERAL (
          SELECT p.id,jsonb_build_object('actor_id',p.publisher_actor_id,'display_name',a.display_name,
            'at_unix_ms',(extract(epoch FROM p.created_at)*1000)::bigint,
            'source_writeback_pending',p.source_writeback_pending) body
          FROM awr_team.planning_publish_receipts p LEFT JOIN awr_team.actors a ON a.tenant_id=p.tenant_id AND a.id=p.publisher_actor_id
          WHERE p.tenant_id=c.tenant_id AND p.project_id=c.project_id AND p.candidate_id=c.id
            AND p.candidate_digest=c.candidate_digest AND p.draft_revision=c.draft_revision
          ORDER BY p.created_at DESC,p.id DESC LIMIT 1) publication ON true
        LEFT JOIN LATERAL (
          SELECT jsonb_build_object('actor_id',p.publisher_actor_id,'display_name',a.display_name,
            'at_unix_ms',(extract(epoch FROM p.created_at)*1000)::bigint,
            'activated_snapshot_id',p.activated_snapshot_id,'matches_current_snapshot',p.activated_snapshot_id=$3) body
          FROM awr_team.planning_activation_receipts p LEFT JOIN awr_team.actors a ON a.tenant_id=p.tenant_id AND a.id=p.publisher_actor_id
          WHERE p.tenant_id=c.tenant_id AND p.project_id=c.project_id AND p.candidate_id=c.id
            AND p.candidate_digest=c.candidate_digest AND p.publish_receipt_id=publication.id
          ORDER BY p.created_at DESC,p.id DESC LIMIT 1) activation ON true
        LEFT JOIN LATERAL (
          SELECT jsonb_build_object('actor_id',p.reviewer_actor_id,'display_name',a.display_name,
            'at_unix_ms',(extract(epoch FROM p.decided_at)*1000)::bigint,'reason',p.reason) body
          FROM awr_team.planning_rejections p LEFT JOIN awr_team.actors a ON a.tenant_id=p.tenant_id AND a.id=p.reviewer_actor_id
          WHERE p.tenant_id=c.tenant_id AND p.project_id=c.project_id AND p.candidate_id=c.id
            AND p.candidate_digest=c.candidate_digest AND p.draft_revision=c.draft_revision) rejection ON true
        WHERE {FILTER} AND c.id>$9 ORDER BY c.id LIMIT $10"),&page_params).await?;
    let changes: Vec<Vec<DraftChange>> = rows
        .iter()
        .take(limit as usize)
        .map(|r| serde_json::from_value(r.get(9)).map_err(|_| PgError::SourceDivergence))
        .collect::<PgResult<_>>()?;
    let ids: Vec<String> = changes
        .iter()
        .flatten()
        .flat_map(|c| {
            std::iter::once(c.after.work_id.clone()).chain(c.after.required_dependencies.clone())
        })
        .collect();
    let records = tx
        .query(
            "SELECT c.work_id,c.contract_json,c.contract_hash,c.title,c.definition_state
        FROM awr_team.work_contracts c JOIN awr_team.workstream_snapshot_ownership o
          USING(tenant_id,project_id,snapshot_id,scope_id,work_id)
        WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.snapshot_id=$3
          AND o.workstream_id=ANY($4) AND c.work_id=ANY($5)",
            &[&tenant, &project, &auth.snapshot, &streams, &ids],
        )
        .await?;
    let mut current = BTreeMap::new();
    for row in records {
        let id: String = row.get(0);
        let mut value: Value = row.get(1);
        let contract: WorkContract =
            serde_json::from_value(value.clone()).map_err(|_| PgError::SourceDivergence)?;
        if contract.work_id.as_str() != id
            || contract.hash().map_err(|_| PgError::SourceDivergence)? != row.get::<_, String>(2)
        {
            return Err(PgError::SourceDivergence);
        }
        value["title"] = json!(row.get::<_, String>(3));
        value["definition_state"] = json!(row.get::<_, String>(4));
        current.insert(id, value);
    }
    let references: Vec<String> = current
        .values()
        .flat_map(|v| {
            v["required_dependencies"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
        })
        .chain(
            changes
                .iter()
                .flatten()
                .flat_map(|c| c.after.required_dependencies.clone()),
        )
        .collect();
    let mut readable: BTreeSet<String> = tx.query("SELECT work_id FROM awr_team.workstream_snapshot_ownership
        WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND workstream_id=ANY($4) AND work_id=ANY($5)",
        &[&tenant,&project,&auth.snapshot,&streams,&references]).await?.into_iter().map(|r|r.get(0)).collect();
    readable.extend(changes.iter().flatten().map(|c| c.after.work_id.clone()));
    let items=rows.iter().zip(changes).map(|(r,changes)| {
        let differences=changes.iter().map(|c|diff(c,current.get(&c.after.work_id),&readable))
            .collect::<PgResult<Vec<_>>>()?;
        let counts: BTreeMap<&str,usize>=["added","removed","changed","unchanged"].into_iter()
            .map(|kind|(kind,differences.iter().filter(|d|d["kind"]==kind).count())).collect();
        Ok(json!({"candidate_id":r.get::<_,String>(0),"candidate_digest":r.get::<_,String>(6),
            "draft_revision":r.get::<_,i32>(7),"state":r.get::<_,String>(8),
            "author":{"actor_id":r.get::<_,String>(1),"display_name":r.get::<_,Option<String>>(2),
                "person_id":r.get::<_,String>(3)},
            "created_at_unix_ms":r.get::<_,i64>(10),"updated_at_unix_ms":r.get::<_,i64>(11),
            "baseline":{"source_manifest_digest":r.get::<_,String>(4),"authority_epoch":r.get::<_,String>(5),
                "matches_current":stamp.get::<_,Option<String>>(4).as_deref()==Some(r.get::<_,String>(4).as_str())
                    && r.get::<_,String>(5)==stamp.get::<_,String>(3)},
            "current_diff":{"items":differences,"counts":counts},
            "approval":nullable(r.get::<_,Option<Value>>(12).unwrap_or(Value::Null),"not_approved"),
            "publication":nullable(r.get::<_,Option<Value>>(13).unwrap_or(Value::Null),"not_published"),
            "activation":nullable(r.get::<_,Option<Value>>(14).unwrap_or(Value::Null),"not_applied"),
            "rejection":nullable(r.get::<_,Option<Value>>(15).unwrap_or(Value::Null),"not_rejected")}))
    }).collect::<PgResult<Vec<_>>>()?;
    let next = if rows.len() > limit as usize {
        next_cursor(
            &binding,
            items.last().unwrap()["candidate_id"].as_str().unwrap(),
            0,
            -1,
        )
    } else {
        Value::Null
    };
    let mut result = json!({"protocol_version":1,"scope_id":"main","selection_basis":"project",
        "project_revision":auth.revision.to_string(),"source_snapshot_id":auth.snapshot,
        "coordinator_epoch":auth.epoch,"project_status":auth.project_status,
        "data":{"items":items,"total":stamp.get::<_,i64>(0),"next_cursor":next,
            "truncated":rows.len()>limit as usize,"as_of":{"at_unix_ms":stamp.get::<_,i64>(2),
                "project_revision":auth.revision.to_string(),"source_snapshot_id":auth.snapshot,
                "planning_revision":planning_revision},"visibility":"fully_readable_candidates",
            "execution_authorized":false}});
    if let Some(stream) = selected {
        result["workstream_id"] = json!(stream.id.to_string());
        result["authority_version"] = json!(stream.authority_version.to_string());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omitted_optional_assurance_and_settlement_retain_current_values() {
        let current = json!({"work_id":"work","external_key":"WORK","title":"Update help",
            "definition_state":"enabled","goals":["goal"],"required_dependencies":["input"],
            "scope_paths":["docs"],"acceptance":["Help explains the current behavior"],
            "completion_policy":"caller_managed_execution_and_agent_review",
            "dependency_acceptance":{"input":"agent_reviewed_caller_asserted_reconciled"},
            "execution_settlement":{"mode":"independent_workspace_v1","workspace_id":"help-workspace"}});
        let mut proposed = current.clone();
        proposed
            .as_object_mut()
            .unwrap()
            .remove("dependency_acceptance");
        proposed
            .as_object_mut()
            .unwrap()
            .remove("execution_settlement");
        let change: DraftChange = serde_json::from_value(json!({
            "op":"edit_fields","before":current,"after":proposed
        }))
        .unwrap();
        let observed = diff(&change, Some(&current), &BTreeSet::from(["input".into()])).unwrap();
        assert_eq!(observed["kind"], "unchanged");
        assert_eq!(observed["fields"], json!([]));
    }
}
