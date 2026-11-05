use super::*;
use crate::workstream_auth::authorize_domain_action;

pub(super) fn identity(auth: &ReaderAuthority) -> Value {
    let manages = auth.access.grants.iter().any(|g| {
        auth.access
            .authorize(&auth.catalog, g.workstream_id, WorkstreamAction::Manage)
            .is_ok()
    });
    let mut identity = json!({"actor_id":auth.actor_id,"client_id":auth.client_id,"role":auth.role,
        "membership_action_ceiling":auth.membership_actions(),
        "can_manage_members":manages && authorize_domain_action(auth, awr_team::Action::AccessManageProject,None,None).is_ok(),
        "can_read_project_audit":authorize_domain_action(auth, awr_team::Action::AuditReadProject,None,None).is_ok()});
    if let Some(roles) = &auth.business_roles {
        identity["business_roles"] = json!(roles);
    }
    identity
}

pub(super) fn visible_streams(auth: &ReaderAuthority) -> Vec<String> {
    auth.catalog
        .workstreams
        .iter()
        .filter(|s| {
            auth.access
                .authorize(&auth.catalog, s.id, WorkstreamAction::Read)
                .is_ok()
        })
        .map(|s| s.id.to_string())
        .collect()
}

pub(super) async fn next(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    q: &WorkstreamQuery,
) -> PgResult<Value> {
    let (streams, task_ids, task_streams): (Vec<String>, Vec<String>, Vec<String>) =
        match &auth.navigation_read_scope {
            Some(scope) => (
                scope.streams.iter().map(ToString::to_string).collect(),
                scope.tasks.keys().cloned().collect(),
                scope.tasks.values().map(ToString::to_string).collect(),
            ),
            None => (visible_streams(auth), Vec::new(), Vec::new()),
        };
    let binding = hash(
        &json!({"reader":auth.binding,"snapshot":auth.snapshot,"catalog":auth.catalog,
        "grants":auth.grant_versions,"op":"work.next"}),
    )?;
    let c = cursor(q, &binding)?;
    let limit = i64::from(q.limit.unwrap_or(20));
    // Definitions can be inactive before any runtime exists. Keep those out of
    // candidate selection; explicit observation remains available.
    let rows = tx.query("SELECT c.work_id,c.title,c.contract_json,o.workstream_id,r.state,r.recovery_blocked,o.ownership_version,c.definition_state
        FROM awr_team.work_contracts c JOIN awr_team.workstream_snapshot_ownership o
          USING(tenant_id,project_id,snapshot_id,scope_id,work_id)
        LEFT JOIN awr_team.work_runtime r USING(tenant_id,project_id,scope_id,work_id)
        WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.snapshot_id=$3
          AND (o.workstream_id=ANY($4) OR EXISTS (
            SELECT 1 FROM unnest($7::text[], $8::text[]) allowed(work_id,workstream_id)
            WHERE allowed.work_id=c.work_id AND allowed.workstream_id=o.workstream_id))
          AND c.definition_state='enabled'
          AND c.work_id>$5 AND (r.state IS NULL OR r.state NOT IN ('completed','cancelled','archived'))
        ORDER BY c.work_id LIMIT $6", &[&tenant,&project,&auth.snapshot,&streams,&c.key,&(limit+1),&task_ids,&task_streams]).await?;
    let mut resume = Vec::new();
    let mut after_session = String::new();
    loop {
        let own = tx.query("SELECT s.id,s.work_id,s.workstream_id,s.session_version,o.ownership_version,c.definition_state FROM awr_team.sessions s
        JOIN awr_team.workstream_snapshot_ownership o ON o.tenant_id=s.tenant_id AND o.project_id=s.project_id
          AND o.snapshot_id=$3 AND o.work_id=s.work_id AND o.workstream_id=s.workstream_id AND o.ownership_version=s.ownership_version
        JOIN awr_team.work_contracts c ON c.tenant_id=s.tenant_id AND c.project_id=s.project_id
          AND c.snapshot_id=$3 AND c.scope_id=s.scope_id AND c.work_id=s.work_id
        WHERE s.tenant_id=$1 AND s.project_id=$2 AND s.actor_id=$4 AND s.client_id=$5 AND s.state='active'
          AND s.id>$9
          AND (s.workstream_id=ANY($6) OR EXISTS (
            SELECT 1 FROM unnest($7::text[], $8::text[]) allowed(work_id,workstream_id)
            WHERE allowed.work_id=s.work_id AND allowed.workstream_id=s.workstream_id))
          ORDER BY s.id LIMIT 21", &[&tenant,&project,&auth.snapshot,&auth.actor_id,&auth.client_id,&streams,&task_ids,&task_streams,&after_session]).await?;
        for s in &own {
            let session: String = s.get(0);
            let work: String = s.get(1);
            let stream: String = s.get(2);
            after_session.clone_from(&session);
            if s.get::<_, String>(5) != "enabled" {
                // Reuse current guidance so recovery and unresolved effects keep
                // priority without duplicating their predicates in navigation.
                let current = super::observation::read(
                    tx,
                    tenant,
                    project,
                    auth,
                    &work,
                    &stream,
                    s.get(4),
                    Some(&session),
                )
                .await?;
                if current["guidance"]["action"]["op"] == "work.next" {
                    continue;
                }
            }
            resume.push(json!({"session_id":session,"work_id":work,
        "workstream_id":stream,"session_version":s.get::<_,i64>(3).to_string(),
        "next_query":{"protocol_version":1,"op":"work.observe","session_id":session,
            "work_id":work,"workstream_id":stream}}));
            if resume.len() > 20 {
                break;
            }
        }
        // Apply the bound after filtering; inactive history must not hide a
        // later eligible session or make resume_truncated inaccurate.
        if resume.len() > 20 || own.len() < 21 {
            break;
        }
    }
    let resume_truncated = resume.len() > 20;
    resume.truncate(20);
    let mut items = Vec::new();
    for row in rows.iter().take(limit as usize) {
        let work: String = row.get(0);
        let stream: String = row.get(3);
        let contract: WorkContract =
            serde_json::from_value(row.get(2)).map_err(|_| PgError::SourceDivergence)?;
        contract.validate().map_err(|_| PgError::SourceDivergence)?;
        let blocked_deps = !crate::workstream_command::task_intake::dependencies_ready(
            tx, tenant, project, auth, &work, &stream,
        )
        .await?;
        let claims=tx.query("SELECT c.expires_at>clock_timestamp(),c.actor_id,s.client_id FROM awr_team.claims c JOIN awr_team.sessions s ON s.tenant_id=c.tenant_id AND s.project_id=c.project_id AND s.id=c.session_id
            WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.work_id=$3 AND c.state='active'", &[&tenant,&project,&work]).await?;
        let held = claims.iter().any(|c| {
            c.get::<_, bool>(0)
                && (c.get::<_, String>(1) != auth.actor_id
                    || c.get::<_, String>(2) != auth.client_id)
        });
        let expired = claims.iter().any(|c| !c.get::<_, bool>(0));
        let scoped = crate::delegation_auth::navigation_authority(
            auth,
            awr_team::Action::ClaimManageOwn,
            stream.parse().map_err(|_| PgError::SourceDivergence)?,
            &work,
        );
        let responsibility = crate::workstream_command::task_intake::read_state(
            tx,
            tenant,
            project,
            scoped.as_ref().unwrap_or(auth),
            &work,
            &stream,
            None,
        )
        .await?;
        let writable = auth
            .access
            .authorize(
                &auth.catalog,
                stream.parse().map_err(|_| PgError::SourceDivergence)?,
                WorkstreamAction::Write,
            )
            .is_ok()
            && scoped.is_ok();
        let pending = tx.query_one("SELECT
            EXISTS(SELECT 1 FROM awr_team.wait_items WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='open'),
            (SELECT count(*) FROM (SELECT 1 FROM awr_team.executions WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state NOT IN ('succeeded','failed','cancelled') LIMIT 2) unsettled),
            EXISTS(SELECT 1 FROM awr_team.resource_reservations WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='unknown')", &[&tenant,&project,&work]).await?;
        let recovery_blocked = row.get::<_, Option<bool>>(5).unwrap_or(false);
        let unsettled: i64 = pending.get(1);
        let unknown_resources: bool = pending.get(2);
        let can_continue = writable
            && crate::delegation_auth::authorize_navigation_action(
                auth,
                awr_team::Action::ExecutionRequestAndReportOwn,
                stream.parse().map_err(|_| PgError::SourceDivergence)?,
                &work,
            )
            .is_ok();
        let resumable = if unsettled == 1
            && !pending.get::<_, bool>(0)
            && !unknown_resources
            && !recovery_blocked
            && (!expired || responsibility["current_executor_matches_client"] == true)
            && !held
            && !blocked_deps
            && can_continue
        {
            // Reuse the current observation's binding and lease checks. A
            // live run owned by this client is not an unresolved recovery.
            let current = super::observation::read(
                tx,
                tenant,
                project,
                auth,
                &work,
                &stream,
                row.get(6),
                None,
            )
            .await?;
            let execution = &current["execution"];
            execution["state"] == "running"
                && execution["owned_by_client"] == true
                && execution["lease_live"] == true
                && execution["epoch_matches_current"] == true
                && execution["contract_matches_current"] == true
                && execution["cancel_requested"] == false
        } else {
            false
        };
        let status = if resumable {
            "resume"
        } else if recovery_blocked || unsettled > 0 || unknown_resources {
            "recovery_required"
        } else if matches!(
            responsibility["relation"].as_str(),
            Some("handoff_required" | "responsibility_pending")
        ) {
            "handoff_required"
        } else if matches!(
            responsibility["relation"].as_str(),
            Some("owned_by_other" | "assigned_to_other")
        ) {
            "held"
        } else if expired && responsibility["current_executor_matches_client"] != true {
            "recovery_required"
        } else if held {
            "held"
        } else if pending.get::<_, bool>(0) {
            "waiting_external"
        } else if blocked_deps {
            "waiting_dependency"
        } else if !writable {
            "observe"
        } else {
            "prepare"
        };
        items.push(json!({"work_id":work,"title":row.get::<_,String>(1),"workstream_id":stream,
            "state":row.get::<_,Option<String>>(4),"definition_state":row.get::<_,String>(7),"navigation":status,"responsibility":responsibility,
            "next_query":{"protocol_version":1,"op":if matches!(status,"resume"|"observe") {"work.observe"} else if matches!(status,"recovery_required"|"waiting_external") {"work.recovery"} else {"work.prepare"},"work_id":work,"workstream_id":stream}}));
    }
    let next = if rows.len() > limit as usize {
        next_cursor(
            &binding,
            items.last().unwrap()["work_id"].as_str().unwrap(),
            0,
            -1,
        )
    } else {
        Value::Null
    };
    Ok(
        json!({"protocol_version":1,"project_revision":auth.revision.to_string(),"data":{
        "identity":identity(auth),"resume":resume,"resume_truncated":resume_truncated,"items":items,"next_cursor":next,
        "execution_authorized":false,"next_action":"Follow current guidance for the listed own sessions first; otherwise follow the selected item's next_query. Stop automatic navigation on inspect_only until recheck_on changes. Candidates have enabled definitions; consume work.prepare before session/claim/execution admission. Navigation is advisory, not a reservation or dependency acceptance.",
        "recheck_on":["progress","wait_resolved","claim_conflict","source_change","permission_change","unknown_outcome"]}}),
    )
}
