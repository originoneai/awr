//! Current actionable conditions, projected from durable facts without a second ledger.
use super::*;
use awr_team::Action;
use awr_team::delivery::{DeliveryCandidate, DeliveryEnvelope, DeliveryRecord};

/// Read one work's compact collaboration facts in the same authenticated snapshot.
/// Provider history, credential bindings and caller-declared narrative stay out.
pub(super) async fn facts(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    work: &str,
    contract: &str,
    ownership: i64,
) -> PgResult<Value> {
    let (binding, _) = work_binding(tx, tenant, project, auth, work).await?;
    authorize_query(auth, Some(binding.workstream_id), Some(work), "work.inbox")?;
    let barriers = tx.query_one("SELECT
        EXISTS(SELECT 1 FROM awr_team.executions e WHERE e.tenant_id=$1 AND e.project_id=$2 AND e.work_id=$3
          AND e.state NOT IN ('succeeded','failed','cancelled') AND (e.state='unknown'
            OR e.contract_hash<>$4 OR e.coordinator_epoch IS DISTINCT FROM $5
            OR NOT EXISTS(SELECT 1 FROM awr_team.claims c WHERE c.tenant_id=e.tenant_id AND c.project_id=e.project_id
              AND c.id=e.claim_id AND c.work_id=e.work_id AND c.session_id=e.session_id AND c.fence=e.fence
              AND c.state='active' AND c.expires_at>clock_timestamp() AND c.coordinator_epoch=$5))),
        EXISTS(SELECT 1 FROM awr_team.resource_reservations WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='unknown'),
        EXISTS(SELECT 1 FROM awr_team.claims WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='active'
          AND (expires_at<=clock_timestamp() OR coordinator_epoch IS DISTINCT FROM $5)),
        EXISTS(SELECT 1 FROM awr_team.claims WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='active')",
        &[&tenant,&project,&work,&contract,&auth.epoch]).await?;
    let mut data = json!({"review":null,"candidate":null,"verification":[],
        "recovery":{"unsettled_execution":barriers.get::<_,bool>(0),"unknown_resource":barriers.get::<_,bool>(1),
            "expired_claim":barriers.get::<_,bool>(2)},"claim_present":barriers.get::<_,bool>(3),
        "facts_truncated":false,"integration":null,"publication":null,
        "acceptance_inferred":false,"source_synchronized":false});
    let round = tx
        .query_opt(
            "SELECT id,state,bundle_hash,author_actor_id FROM awr_team.review_rounds
        WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND contract_hash=$4
        ORDER BY round_index DESC,id DESC LIMIT 1",
            &[&tenant, &project, &work, &contract],
        )
        .await?;
    if let Some(r) = round {
        data["review"] = json!({"id":r.get::<_,String>(0),"state":r.get::<_,String>(1),
            "bundle_hash":r.get::<_,String>(2),"author_actor_id":r.get::<_,String>(3)});
    }
    let selected = tx.query_opt("SELECT s.binding_digest,c.body_json,s.selection_version,
        s.source_snapshot_id=$4 AND s.ownership_version=$5 AND w.last_fence=s.fence
          AND cl.coordinator_epoch=$6
        FROM awr_team.delivery_selections s JOIN awr_team.delivery_candidates c
          USING(tenant_id,project_id,binding_digest)
        JOIN awr_team.work_runtime w ON (w.tenant_id,w.project_id,w.scope_id,w.work_id)=(s.tenant_id,s.project_id,s.scope_id,s.work_id)
        JOIN awr_team.claims cl ON (cl.tenant_id,cl.project_id,cl.id)=(s.tenant_id,s.project_id,s.claim_id)
        WHERE s.tenant_id=$1 AND s.project_id=$2 AND s.work_id=$3 AND s.scope_id='main'",
        &[&tenant,&project,&work,&auth.snapshot,&ownership,&auth.epoch]).await?;
    if let Some(s) = selected {
        let digest: String = s.get(0);
        let candidate: DeliveryCandidate =
            serde_json::from_value(s.get(1)).map_err(|_| PgError::SourceDivergence)?;
        if candidate
            .binding
            .digest()
            .map_err(|_| PgError::SourceDivergence)?
            != digest
            || candidate.binding.work_id.as_str() != work
            || candidate.binding.tenant_id.as_str() != tenant
            || candidate.binding.project_id.as_str() != project
            || candidate.binding.scope_id.as_str() != "main"
            || candidate.binding.workstream_id != binding.workstream_id.to_string()
        {
            return Err(PgError::SourceDivergence);
        }
        let version: i64 = s.get(2);
        let current = s.get::<_, Option<bool>>(3).unwrap_or(false)
            && candidate.binding.contract_hash == contract;
        data["candidate"] = json!({"digest":digest,"selection_version":version.to_string(),
            "current":current,"required_checks":candidate.binding.required_checks});
        if current {
            let rows = tx.query("SELECT f.envelope_json FROM awr_team.delivery_fact_heads h
                JOIN awr_team.delivery_facts f ON (f.tenant_id,f.project_id,f.id)=(h.tenant_id,h.project_id,h.fact_id)
                JOIN awr_team.delivery_inbox i ON (i.tenant_id,i.project_id,i.id)=(f.tenant_id,f.project_id,f.inbox_id)
                JOIN awr_team.delivery_inspections x ON (x.tenant_id,x.project_id,x.id)=(i.tenant_id,i.project_id,i.inspection_id)
                JOIN awr_team.delivery_connectors c ON (c.tenant_id,c.project_id,c.id)=(h.tenant_id,h.project_id,h.connector_id)
                WHERE h.tenant_id=$1 AND h.project_id=$2 AND h.work_id=$3 AND x.binding_digest=$4
                  AND x.selection_version=$5 AND x.source_snapshot_id=$6 AND x.ownership_version=$7
                  AND x.coordinator_epoch=$8 AND c.version=x.connector_version AND c.enabled
                  AND c.coordinator_epoch=x.coordinator_epoch
                ORDER BY h.connector_id,h.slot LIMIT 33",
                &[&tenant,&project,&work,&digest,&version,&auth.snapshot,&ownership,&auth.epoch]).await?;
            data["facts_truncated"] = json!(rows.len() > 32);
            let mut checks = Vec::new();
            for row in rows.iter().take(32) {
                let envelope: DeliveryEnvelope =
                    serde_json::from_value(row.get(0)).map_err(|_| PgError::SourceDivergence)?;
                if envelope
                    .record
                    .binding()
                    .is_some_and(|b| b != &candidate.binding)
                {
                    return Err(PgError::SourceDivergence);
                }
                if let DeliveryRecord::Verification(r) = envelope.record {
                    checks.push(json!({"check":r.check,"run_id":r.run_id,"outcome":r.outcome}));
                }
            }
            data["verification"] = json!(checks);
        }
        let publication = tx
            .query_opt(
                "SELECT id,phase,metadata_revision FROM awr_team.delivery_source_publications
            WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND candidate_digest=$4
              AND selection_version=$5 AND source_snapshot_id=$6
            ORDER BY metadata_revision DESC,id DESC LIMIT 1",
                &[&tenant, &project, &work, &digest, &version, &auth.snapshot],
            )
            .await?;
        if let Some(p) = publication {
            data["publication"] = json!({"id":p.get::<_,String>(0),"phase":p.get::<_,String>(1),
                "metadata_revision":p.get::<_,i64>(2).to_string(),"basis":"recorded_publication_phase"});
        }
    }
    // Historical unsettled effects must remain visible even after reselection.
    let integration = tx
        .query_opt(
            "SELECT id,state FROM awr_team.delivery_integration_intents
        WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3
        ORDER BY (state IN ('dispatched','unknown')) DESC,created_at DESC,id DESC LIMIT 1",
            &[&tenant, &project, &work],
        )
        .await?;
    if let Some(i) = integration {
        data["integration"] = json!({"id":i.get::<_,String>(0),"state":i.get::<_,String>(1)});
    }
    Ok(data)
}

fn permits(auth: &ReaderAuthority, action: Action, op: &str, stream: Id, work: &str) -> bool {
    crate::delegation_auth::navigation_authority(auth, action, stream, work).is_ok_and(|scoped| {
        [
            crate::workstream_auth::CommandAuthPhase::Admission,
            crate::workstream_auth::CommandAuthPhase::Effect,
        ]
        .into_iter()
        .all(|phase| {
            crate::workstream_auth::authorize_command(&scoped, stream, work, op, phase).is_ok()
        })
    })
}

/// One current condition and one authorized inspection. Commands reauthorize
/// their own admission; an inbox item is never a decision or reservation.
fn select(
    data: &Value,
    supervisor: bool,
    developer: bool,
    reviewer: bool,
    deliverer: bool,
    dependencies_ready: bool,
) -> Option<Value> {
    let c = &data["collaboration"];
    let relation = data["responsibility"]["relation"]
        .as_str()
        .unwrap_or("unknown");
    let own = matches!(
        relation,
        "assigned_to_me" | "owned_by_me" | "handoff_required"
    );
    let involved = supervisor || reviewer || deliverer || (developer && own);
    let phase = if data["progress"]["contract_matches_current"] == true
        && data["progress"]["stale"] == false
    {
        data["progress"]["phase"].as_str()
    } else {
        None
    };
    let completed = data["runtime"]["state"] == "completed";
    let closed = matches!(
        data["runtime"]["state"].as_str(),
        Some("completed" | "cancelled" | "archived")
    );
    let (code, when, basis, op, note) = if data["runtime"]["recovery_blocked"] == true
        || data["execution"]["state"] == "unknown"
        || c["recovery"]
            .as_object()
            .is_some_and(|r| r.values().any(|v| v == true))
    {
        (
            "recovery",
            "execution or effects are unresolved",
            "a durable recovery barrier remains",
            "work.recovery",
            "Inspect recovery and involve the authorized reconciler before further effects.",
        )
    } else if matches!(
        c["integration"]["state"].as_str(),
        Some("dispatched" | "unknown")
    ) {
        (
            "integration_unknown",
            "repository integration has no confirmed outcome",
            "an integration intent is unsettled",
            "delivery.integration.inspect",
            "Inspect the original integration request; do not submit another repository effect.",
        )
    } else if data["waiting_user"] == true || matches!(phase, Some("blocked" | "waiting_user")) {
        (
            "blocked",
            "a current blocker or user wait remains",
            "current work or structured feedback records the wait",
            "work.observe",
            "Inspect the recorded blocker; refresh after its authorized resolution or a required fact changes.",
        )
    } else if involved
        && !closed
        && matches!(
            c["review"]["state"].as_str(),
            Some("rejected" | "invalidated")
        )
    {
        (
            "rework",
            "the current review was returned or invalidated",
            "the recorded round is no longer approved",
            "review.inspect",
            "Inspect the current return reason and arrange revised evidence and independent review.",
        )
    } else if involved && !closed && !c["candidate"].is_null() && c["candidate"]["current"] != true
    {
        (
            "delivery_changed",
            "the selected delivery binding changed",
            "the selection no longer matches current work",
            "delivery.neutral.inspect",
            "Inspect current candidate facts and refresh the binding before review or delivery.",
        )
    } else if !closed && c["review"]["state"] == "open" && (reviewer || supervisor) {
        (
            "review",
            "a current AWR review is open",
            "the round is bound to this contract",
            "review.inspect",
            "Inspect the exact round; use a currently authorized independent reviewer for its decision.",
        )
    } else if !closed && c["review"]["state"] == "approved" && (deliverer || supervisor) {
        (
            "finalization",
            "an approved round awaits AWR finalization",
            "approval and completion are separate records",
            "review.inspect",
            "Inspect the approved basis, then have the authorized deliverer recheck finalization preconditions.",
        )
    } else if involved && !closed && c["candidate"]["current"] == true {
        (
            "verification",
            "a current neutral candidate needs delivery inspection",
            "checks and acceptance must be assessed separately",
            "delivery.neutral.inspect",
            "Inspect the required checks and current candidate before opening review or preparing integration.",
        )
    } else if involved
        && completed
        && c["candidate"]["current"] == true
        && c["publication"]["phase"] != "confirmed"
    {
        (
            "source_publication",
            "accepted delivery has no recorded source confirmation",
            "AWR acceptance is separate from source publication",
            "delivery.source.status",
            "Inspect source publication and let the configured publisher recover or confirm the existing request.",
        )
    } else if !closed
        && supervisor
        && relation == "pool"
        && dependencies_ready
        && c["claim_present"] != true
    {
        (
            "assignment",
            "enabled work is available for assignment",
            "no responsible member or blocking dependency is recorded",
            "work.prepare",
            "Consume the work context and assign it to an authorized member, or leave it available for self-claim.",
        )
    } else if !closed
        && developer
        && matches!(relation, "pool" | "assigned_to_me")
        && dependencies_ready
        && c["claim_present"] != true
    {
        (
            "intake",
            "available work or your assignment is ready",
            "current responsibility and dependencies allow preparation",
            "work.prepare",
            "Consume context, start your session and take the task through the appropriate atomic intake command.",
        )
    } else if !closed
        && (supervisor || developer && (own || relation == "pool"))
        && !dependencies_ready
    {
        (
            "dependency",
            "required dependency acceptance is missing",
            "the current dependency gate refuses intake",
            "work.prepare",
            "Inspect the consumer's authorized dependency references and arrange the missing accepted version.",
        )
    } else if involved && !closed && phase == Some("ready_for_review") {
        (
            "reported_ready",
            "a member reports readiness without a current review",
            "readiness is caller-declared feedback",
            "work.observe",
            "Inspect the reported artifacts and have the author submit version-bound evidence for independent review.",
        )
    } else {
        return None;
    };
    let mut next = json!({"protocol_version":1,"op":op,"work_id":data["work_id"],"workstream_id":data["workstream_id"]});
    if op == "review.inspect" {
        next["review_round_id"] = c["review"]["id"].clone();
    }
    if op == "delivery.integration.inspect" {
        next["request_id"] = c["integration"]["id"].clone();
    }
    Some(json!({"code":code,"when":when,"because":[basis],
        "action":{"query":next,"note":note},
        "recheck_on":"responsibility, blocker, dependency, candidate, evidence, review, effect, source or permission changes"}))
}

pub(super) async fn read(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    q: &WorkstreamQuery,
) -> PgResult<Value> {
    let (streams, tasks, owners): (Vec<String>, Vec<String>, Vec<String>) =
        match &auth.navigation_read_scope {
            Some(scope) => (
                scope.streams.iter().map(ToString::to_string).collect(),
                scope.tasks.keys().cloned().collect(),
                scope.tasks.values().map(ToString::to_string).collect(),
            ),
            None => (navigation::visible_streams(auth), Vec::new(), Vec::new()),
        };
    let binding = hash(
        &json!({"reader":auth.binding,"snapshot":auth.snapshot,"catalog":auth.catalog,
        "grants":auth.grant_versions,"op":"work.inbox"}),
    )?;
    let page = cursor(q, &binding)?;
    let limit = i64::from(q.limit.unwrap_or(20));
    // Filter before pagination; private tasks never influence ordering or counts.
    let rows = tx.query("SELECT c.work_id,c.title,c.contract_hash,o.workstream_id,o.ownership_version,c.definition_state
        FROM awr_team.work_contracts c JOIN awr_team.workstream_snapshot_ownership o
          USING(tenant_id,project_id,snapshot_id,scope_id,work_id)
        WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.snapshot_id=$3 AND c.work_id>$5
          AND (o.workstream_id=ANY($4) OR EXISTS(SELECT 1 FROM unnest($7::text[],$8::text[]) allowed(work_id,workstream_id)
            WHERE allowed.work_id=c.work_id AND allowed.workstream_id=o.workstream_id))
        ORDER BY c.work_id LIMIT $6", &[&tenant,&project,&auth.snapshot,&streams,&page.key,&(limit+1),&tasks,&owners]).await?;
    let mut items = Vec::new();
    for row in rows.iter().take(limit as usize) {
        let work: String = row.get(0);
        let stream: String = row.get(3);
        let stream_id: Id = stream.parse().map_err(|_| PgError::SourceDivergence)?;
        // Resolve reading separately from the covering grant for each action.
        let reader =
            crate::delegation_auth::navigation_authority(auth, Action::WorkRead, stream_id, &work)?;
        let mut observed = observation::read(
            tx,
            tenant,
            project,
            &reader,
            &work,
            &stream,
            row.get(4),
            None,
        )
        .await?;
        observed["workstream_id"] = json!(stream);
        observed["collaboration"] = facts(
            tx,
            tenant,
            project,
            &reader,
            &work,
            &row.get::<_, String>(2),
            row.get(4),
        )
        .await?;
        let ready = crate::workstream_command::task_intake::dependencies_ready(
            tx, tenant, project, &reader, &work, &stream,
        )
        .await?;
        let enabled = row.get::<_, String>(5) == "enabled";
        let hint = select(
            &observed,
            enabled && permits(auth, Action::WorkAssign, "task.assign", stream_id, &work),
            enabled
                && permits(
                    auth,
                    Action::ClaimManageOwn,
                    "task.claim_available",
                    stream_id,
                    &work,
                ),
            permits(
                auth,
                Action::ReviewDecide,
                "review.decide",
                stream_id,
                &work,
            ) && observed["collaboration"]["review"]["author_actor_id"] != auth.actor_id,
            permits(
                auth,
                Action::DeliveryFinalize,
                "delivery.finalize",
                stream_id,
                &work,
            ),
            ready,
        );
        if let Some(hint) = hint {
            let semantic = json!({"work":work,"contract":observed["contract_hash"],"code":hint["code"],
                "responsibility":observed["responsibility"]["version"],"runtime":observed["runtime"],
                "progress":{"phase":observed["progress"]["phase"],"blockers":observed["progress"]["blockers"]},
                "waiting_user":observed["waiting_user"],"collaboration":observed["collaboration"],"dependencies_ready":ready});
            items.push(json!({"item_key":hash(&semantic)?,"work_id":work,"workstream_id":stream,
                "contract_hash":observed["contract_hash"],"title":row.get::<_,String>(1).chars().take(160).collect::<String>(),
                "title_truncated":row.get::<_,String>(1).chars().count()>160,
                "guidance":hint,"next_query":hint["action"]["query"]}));
        }
    }
    let next = if rows.len() > limit as usize {
        next_cursor(
            &binding,
            &rows[limit as usize - 1].get::<_, String>(0),
            0,
            -1,
        )
    } else {
        Value::Null
    };
    let result = json!({"protocol_version":1,"scope_id":"main","source_snapshot_id":auth.snapshot,
        "project_revision":auth.revision.to_string(),"coordinator_epoch":auth.epoch,"data":{
            "identity":navigation::identity(auth),"items":items,"next_cursor":next,
            "state_basis":"current_persistent_facts","deduplicate_by":"item_key",
            "refresh_from_first_page_on_change":true,"execution_authorized":false,
            "next_action":"Follow one item's next_query; recheck current command admission before any effect. Page until next_cursor is null, including empty pages."}});
    if serde_json::to_vec(&result)
        .map_err(|_| PgError::SourceDivergence)?
        .len()
        > q.max_context_bytes.unwrap_or(65_536)
    {
        return Err(PgError::ResponseTooLarge);
    }
    Ok(result)
}
