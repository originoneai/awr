//! Page-sized business summaries from the same records used by work.snapshot.
use super::*;
use std::collections::BTreeMap;

const LIST_PAGE_QUERY: &str = "WITH page AS MATERIALIZED (
        SELECT c.work_id,c.title,c.contract_hash,c.tenant_id,c.project_id,c.scope_id,
          o.workstream_id,o.ownership_version
        FROM awr_team.work_contracts c
        JOIN awr_team.workstream_snapshot_ownership o USING(tenant_id,project_id,snapshot_id,scope_id,work_id)
        WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.snapshot_id=$3 AND o.workstream_id=$4
          AND c.work_id>$5 AND strpos(lower(c.title),lower($6))>0 ORDER BY c.work_id LIMIT $7
        )
        SELECT c.work_id,c.title,c.contract_hash,c.ownership_version,
        r.state,t.owner_person_id,p.display_name,s.id,s.client_info_json,
        s.session_state,s.last_checkpoint_at_unix_ms,
        (extract(epoch FROM statement_timestamp())*1000)::bigint,t.work_id
        FROM page c
        LEFT JOIN awr_team.work_runtime r USING(tenant_id,project_id,scope_id,work_id)
        LEFT JOIN awr_team.task_responsibilities t
          ON (t.tenant_id,t.project_id,t.work_id)=(c.tenant_id,c.project_id,c.work_id)
        LEFT JOIN awr_team.persons p
          ON (p.tenant_id,p.project_id,p.id)=(t.tenant_id,t.project_id,t.owner_person_id)
        LEFT JOIN LATERAL (
          SELECT s.id,s.client_info_json,s.state AS session_state,(extract(epoch FROM cp.created_at)*1000)::bigint AS last_checkpoint_at_unix_ms FROM awr_team.sessions s
          LEFT JOIN awr_team.checkpoints cp
            ON (cp.tenant_id,cp.project_id,cp.session_id,cp.id)=(s.tenant_id,s.project_id,s.id,s.latest_checkpoint_id)
          WHERE s.tenant_id=c.tenant_id AND s.project_id=c.project_id AND s.scope_id=c.scope_id
            AND s.work_id=c.work_id AND s.workstream_id=c.workstream_id AND s.ownership_version=c.ownership_version
          ORDER BY EXISTS(SELECT 1 FROM awr_team.claims cl
            WHERE (cl.tenant_id,cl.project_id,cl.session_id,cl.work_id,cl.scope_id,cl.workstream_id,cl.ownership_version)
              =(s.tenant_id,s.project_id,s.id,s.work_id,s.scope_id,s.workstream_id,s.ownership_version)
              AND cl.coordinator_epoch=$8 AND cl.state='active' AND cl.expires_at>clock_timestamp()) DESC,
            (s.state='active') DESC,cp.created_at DESC NULLS LAST,s.id DESC LIMIT 1
        ) s ON true
        ORDER BY c.work_id";

fn field(value: Value, at: i64, missing: &str) -> Value {
    if value.is_null() {
        json!({"value":null,"as_of_unix_ms":null,"reason":missing})
    } else {
        json!({"value":value,"as_of_unix_ms":at,"reason":null})
    }
}

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
    let term = q.search.clone().unwrap_or_default();
    let total: i64 = tx.query_one("SELECT count(*) FROM awr_team.work_contracts c
        JOIN awr_team.workstream_snapshot_ownership o USING(tenant_id,project_id,snapshot_id,scope_id,work_id)
        WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.snapshot_id=$3 AND o.workstream_id=$4
          AND strpos(lower(c.title),lower($5))>0",
        &[&tenant,&project,&auth.snapshot,&stream,&term]).await?.get(0);
    // Bound the page before metadata joins, including the look-ahead row.
    // Lateral selection retains the same ownership/session as observation::read_for_prepare.
    let rows = tx
        .query(
            LIST_PAGE_QUERY,
            &[
                &tenant,
                &project,
                &auth.snapshot,
                &stream,
                &cursor.key,
                &term,
                &(limit + 1),
                &auth.epoch,
            ],
        )
        .await?;
    let bindings: Vec<(String, i64)> = rows
        .iter()
        .take(limit as usize)
        .map(|r| (r.get(0), r.get(3)))
        .collect();
    let work_ids: Vec<String> = bindings.iter().map(|(id, _)| id.clone()).collect();
    let mut historical = retained::owners(tx, tenant, project, &work_ids).await?;
    let mut reports: BTreeMap<String, _> =
        crate::feedback::report_rows(tx, tenant, project, stream, &bindings, None, "progress")
            .await?
            .into_iter()
            .map(|r| (r.get::<_, String>(10), r))
            .collect();
    let mut integrations = crate::DeliverySyncStore::integration_summaries_in_tx(
        tx, tenant, project, auth, &work_ids, None,
    )
    .await?;
    let items: Vec<_> = rows.iter().take(limit as usize).map(|r| {
        let work: String = r.get(0);
        let contract: String = r.get(2);
        let observed: i64 = r.get(11);
        let has_session = r.get::<_,Option<String>>(7).is_some();
        let client = r.get::<_,Option<Value>>(8).unwrap_or(Value::Null);
        let missing = if has_session {crate::feedback::missing(&client,"progress")} else {"no_session"};
        let report = reports.remove(&work).map(|row|
            crate::feedback::report_value(&row,"progress",&contract,observed)).unwrap_or(Value::Null);
        let reported = report["reported_at_unix_ms"].as_i64().unwrap_or(observed);
        let integration = integrations.remove(&work).unwrap_or(Value::Null);
        let owner = r.get::<_,Option<String>>(5).map(|id|
            json!({"person_id":id,"display_name":r.get::<_,Option<String>>(6)})).unwrap_or(Value::Null);
        let outcome = if report.is_null() && integration.is_null() {Value::Null} else {
            json!({"summary":report["summary"],"completed":report["completed"],"integration":integration})
        };
        let reported_blocker = report["blockers"].as_array().and_then(|items|items.first()).cloned();
        let has_reported_blocker = reported_blocker.is_some();
        let blocker = reported_blocker.map(|text|json!({"kind":"reported","text":text}))
            .unwrap_or_else(||if integration["state"]=="rejected" && integration["current_binding"]==true {
                json!({"kind":"integration_rejected"})
            } else {Value::Null});
        let blocker_missing = if report.is_null() {missing} else {"no_reported_blocker"};
        let report_origin = if report.is_null() {Value::Null} else {
            json!({"origin":report["origin"],"contract_matches_current":report["contract_matches_current"],
                "stale":report["stale"],"provenance":report["provenance"]})
        };
        let owner_missing = if r.get::<_,Option<String>>(12).is_some() {"unassigned"} else {"not_recorded"};
        // Liveness rides the bounded page row and uses its session of record.
        // Only active stays active; all terminal states read as ended.
        let session_state = r.get::<_,Option<String>>(9).map(|state|
            if state == "active" {"active"} else {"ended"});
        let checkpoint = r.get::<_,Option<i64>>(10);
        let execution = match session_state {
            Some(state) => field(
                json!({"session_state":state,"last_checkpoint_at_unix_ms":checkpoint}),
                observed,
                "no_session",
            ),
            None => field(Value::Null, observed, "no_session"),
        };
        json!({"work_id":work,"title":r.get::<_,String>(1),"contract_hash":contract,
            "historical_responsibility":historical.remove(&work),
            "summary":{"version":1,
                "owner":field(owner,observed,owner_missing),
                "phase":field(report["phase"].clone(),reported,missing),
                "runtime":field(json!(r.get::<_,Option<String>>(4)),observed,"no_runtime"),
                "execution":execution,
                "outcome":field(outcome,if integration.is_null(){reported}else{observed},"no_reported_outcome"),
                "blocker":field(blocker,if has_reported_blocker{reported}else{observed},blocker_missing),
                "next_action":field(report["next_action"].clone(),reported,missing),
                "report":field(report_origin,reported,missing)}})
    }).collect();
    let next = if rows.len() > limit as usize {
        next_cursor(
            binding,
            items.last().unwrap()["work_id"].as_str().unwrap(),
            0,
            -1,
        )
    } else {
        Value::Null
    };
    Ok(json!({"items":items,"total":total,"next_cursor":next}))
}

#[cfg(test)]
mod tests {
    use super::LIST_PAGE_QUERY;

    #[test]
    fn list_page_is_bounded_before_metadata_joins() {
        let normalized = LIST_PAGE_QUERY
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let (page, metadata) = normalized
            .split_once(") SELECT c.work_id")
            .expect("The selected page must be an explicit query boundary");
        assert!(page.starts_with("WITH page AS MATERIALIZED ("));
        assert!(page.contains("c.work_id>$5"));
        assert!(page.contains("strpos(lower(c.title),lower($6))>0"));
        assert!(page.contains("ORDER BY c.work_id LIMIT $7"));
        assert!(!page.contains("LEFT JOIN"));
        assert!(metadata.contains("FROM page c"));
        assert!(metadata.contains("LEFT JOIN LATERAL"));
        assert!(!metadata.contains("LIMIT $7"));
    }

    #[test]
    fn execution_liveness_is_batched_and_current_ownership_only() {
        let normalized = LIST_PAGE_QUERY
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        // Liveness rides the one already-bounded page statement. The existing
        // LATERAL must expose the selected session's state and checkpoint time.
        assert!(normalized.contains(
            "SELECT s.id,s.client_info_json,s.state AS session_state,(extract(epoch FROM cp.created_at)*1000)::bigint AS last_checkpoint_at_unix_ms"
        ));
        // Sessions must match the current ownership row exactly; a session
        // left behind by an older ownership version must not count.
        assert!(normalized.contains("s.ownership_version=c.ownership_version"));
        assert!(normalized.contains("s.workstream_id=c.workstream_id"));
        // Preserve the session-of-record ordering shared with observation:
        // live claim, active state, latest checkpoint, then newer session id.
        assert!(normalized.contains("ORDER BY EXISTS(SELECT 1 FROM awr_team.claims cl"));
        assert!(
            normalized.contains(
                "(s.state='active') DESC,cp.created_at DESC NULLS LAST,s.id DESC LIMIT 1"
            )
        );
        // The checkpoint pointer is read for its timestamp only.
        assert!(normalized.contains("s.latest_checkpoint_id"));
    }
}
