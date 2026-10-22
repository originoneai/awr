//! Bounded work observations. These facts neither admit execution nor accept work.
use super::*;

pub(super) async fn read(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    work: &str,
    stream: &str,
    ownership: i64,
    requested_session: Option<&str>,
) -> PgResult<Value> {
    let (data, _) = read_for_prepare(
        tx,
        tenant,
        project,
        auth,
        work,
        stream,
        ownership,
        requested_session,
    )
    .await?;
    Ok(data)
}

/// Return observation facts plus private command inputs. The inputs never enter
/// the observation body and callers attach them only after final guidance wins.
pub(super) async fn read_for_prepare(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    work: &str,
    stream: &str,
    ownership: i64,
    requested_session: Option<&str>,
) -> PgResult<(Value, Option<super::guidance::NextCommandInputs>)> {
    // The caller has resolved current snapshot ownership and WorkRead authority.
    // Do not change work.prepare's context hash or weaken receipt visibility.
    let contract_row = tx
        .query_one(
            "SELECT contract_hash,definition_state FROM awr_team.work_contracts
        WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND scope_id='main' AND work_id=$4",
            &[&tenant, &project, &auth.snapshot, &work],
        )
        .await?;
    let contract: String = contract_row.get(0);
    let definition_state: String = contract_row.get(1);
    let observed: i64 = tx
        .query_one(
            "SELECT (extract(epoch FROM clock_timestamp())*1000)::bigint",
            &[],
        )
        .await?
        .get(0);
    let runtime = tx.query_opt("SELECT state,recovery_blocked,selected_completion_id,work_version,last_fence FROM awr_team.work_runtime
        WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3",
        &[&tenant,&project,&work]).await?.map(|r| json!({"state":r.get::<_,String>(0),
            "recovery_blocked":r.get::<_,bool>(1),"selected_completion_id":r.get::<_,Option<String>>(2),
            "work_version":r.get::<_,i64>(3).to_string(),"last_fence":r.get::<_,i64>(4).to_string()}));
    let mut responsibility = crate::workstream_command::task_intake::read_state(
        tx,
        tenant,
        project,
        auth,
        work,
        stream,
        requested_session,
    )
    .await?;
    // Preserve the Inspector's existing attribution fields while exposing the
    // complete current responsibility/version and pending assignment.
    responsibility["owner_person_id"] = responsibility["owner"].clone();
    responsibility["executor_agent_id"] = responsibility["current_executor"]["agent_id"].clone();
    responsibility["owner_name"] = tx
        .query_opt(
            "SELECT display_name FROM awr_team.persons
        WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[
                &tenant,
                &project,
                &responsibility["owner_person_id"].as_str(),
            ],
        )
        .await?
        .map_or(Value::Null, |r| json!(r.get::<_, String>(0)));
    let session = tx.query_opt("SELECT s.id,s.actor_id,a.display_name,s.client_id,s.state,c.id,
          c.contract_hash,c.next_action,c.open_loops_json,(extract(epoch FROM c.created_at)*1000)::bigint,
          s.session_version
        FROM awr_team.sessions s LEFT JOIN awr_team.actors a ON a.tenant_id=s.tenant_id AND a.id=s.actor_id
        LEFT JOIN awr_team.checkpoints c ON c.tenant_id=s.tenant_id AND c.project_id=s.project_id
          AND c.session_id=s.id AND c.id=s.latest_checkpoint_id
        WHERE s.tenant_id=$1 AND s.project_id=$2 AND s.scope_id='main' AND s.work_id=$3
          AND s.workstream_id=$4 AND s.ownership_version=$5 AND ($6::text IS NULL OR s.id=$6)
        ORDER BY EXISTS(SELECT 1 FROM awr_team.claims cl WHERE cl.tenant_id=s.tenant_id AND cl.project_id=s.project_id
          AND cl.session_id=s.id AND cl.work_id=s.work_id AND cl.scope_id=s.scope_id AND cl.workstream_id=s.workstream_id
          AND cl.ownership_version=s.ownership_version AND cl.coordinator_epoch=$7 AND cl.state='active'
          AND cl.expires_at>clock_timestamp()) DESC, (s.state='active') DESC,
          c.created_at DESC NULLS LAST, s.id DESC LIMIT 1",
        &[&tenant,&project,&work,&stream,&ownership,&requested_session,&auth.epoch]).await?;
    let mut selected_session_id = None;
    let mut selected_session_version = None;
    let mut selected_claim = None;
    let mut data = json!({"work_id":work,"workstream_id":stream,"contract_hash":contract,"observed_at_unix_ms":observed,
        "definition_state":definition_state,
        "runtime":runtime,"responsibility":responsibility,"session":null,"checkpoint":null,"claim":null,
        "execution":null,"session_execution":null,"last_participation":null,
        "observation_scope":if requested_session.is_some(){"requested_session"}else{"current_work_ownership"},
        "pr_deliveries":[],"client":null,"progress":null,"model":null,"usage":null,
        "missing":{"model":"no_session","usage":"no_session","progress":"no_session"},
        "execution_authorized":false,"automatic_resume":false});
    if let Some(s) = session {
        let session_id: String = s.get(0);
        selected_session_id = Some(session_id.clone());
        selected_session_version = Some(s.get::<_, i64>(10).to_string());
        data["session"] = json!({"id":session_id,"actor_id":s.get::<_,String>(1),
            "actor_name":s.get::<_,Option<String>>(2),"client_id":s.get::<_,String>(3),"state":s.get::<_,String>(4)});
        if let Some(checkpoint) = s.get::<_, Option<String>>(5) {
            data["checkpoint"] = json!({"id":checkpoint,"contract_matches_current":s.get::<_,Option<String>>(6).as_deref()==Some(&contract),
                "next_action":s.get::<_,Option<String>>(7),"open_loops":s.get::<_,Option<Value>>(8),
                "created_at_unix_ms":s.get::<_,Option<i64>>(9)});
        }
        crate::feedback::observe(
            tx,
            tenant,
            project,
            &session_id,
            work,
            stream,
            ownership,
            requested_session,
            &contract,
            observed,
            &mut data,
        )
        .await?;
        let claim = tx.query_opt("SELECT id,state,expires_at>clock_timestamp(),coordinator_epoch,
              (extract(epoch FROM expires_at)*1000)::bigint,fence,lease_version
            FROM awr_team.claims WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3
              AND workstream_id=$4 AND ownership_version=$5 AND session_id=$6
            ORDER BY acquired_at DESC,id DESC LIMIT 1", &[&tenant,&project,&work,&stream,&ownership,&session_id]).await?
            ;
        data["claim"] = if let Some(r) = claim {
            let current = r.get::<_, Option<String>>(3).as_deref() == Some(&auth.epoch);
            let claim_id = r.get::<_, String>(0);
            selected_claim = Some((
                claim_id.clone(),
                r.get::<_, i64>(5).to_string(),
                r.get::<_, i64>(6).to_string(),
            ));
            json!({"id":claim_id,"state":r.get::<_,String>(1),
                "lease_live":r.get::<_,String>(1)=="active" && r.get::<_,bool>(2) && current,
                "epoch_matches_current":current,"expires_at_unix_ms":r.get::<_,i64>(4)})
        } else {
            Value::Null
        };
        if let Some(e) = tx.query_opt("SELECT id FROM awr_team.executions WHERE tenant_id=$1 AND project_id=$2
            AND scope_id='main' AND work_id=$3 AND workstream_id=$4 AND ownership_version=$5 AND session_id=$6
            ORDER BY id DESC LIMIT 1", &[&tenant,&project,&work,&stream,&ownership,&session_id]).await? {
            let execution = crate::workstream_command::executions::inspect(
                tx,tenant,project,auth,work,stream,ownership,&e.get::<_,String>(0),Some(&session_id)).await?;
            // Only guidance inputs are repeated, never raw receipt payloads.
            let mut summary = serde_json::Map::new();
            for key in ["execution_id", "state", "effects_settled", "contract_matches_current",
                "epoch_matches_current", "lease_live", "owned_by_client", "cancel_requested",
                "recovery_blocked", "previous_epoch_review_required", "controlled_confirmation_available"] {
                summary.insert(key.into(), execution[key].clone());
            }
            // Guidance only needs presence; the full declaration stays on execution.
            summary.insert("terminal_reporting".into(), if execution["terminal_reporting"].is_object() {
                json!({})
            } else { Value::Null });
            data["session_execution"] = Value::Object(summary);
            data["execution"] = execution;
        }
    }
    // Select execution independently: a newer reviewer session must not hide
    // the developer's result. The existing inspector still filters raw receipts.
    if let Some(e) = tx.query_opt("SELECT e.id,e.session_id,e.executor_actor_id,e.executor_client_id,a.display_name
        FROM awr_team.executions e LEFT JOIN awr_team.actors a
          ON a.tenant_id=e.tenant_id AND a.id=e.executor_actor_id
        WHERE e.tenant_id=$1 AND e.project_id=$2 AND e.scope_id='main' AND e.work_id=$3
          AND e.workstream_id=$4 AND e.ownership_version=$5 AND ($6::text IS NULL OR e.session_id=$6)
        ORDER BY e.id DESC LIMIT 1", &[&tenant,&project,&work,&stream,&ownership,&requested_session]).await? {
        let id: String = e.get(0);
        if data["execution"]["execution_id"].as_str() != Some(&id) {
            data["execution"] = crate::workstream_command::executions::inspect(
                tx,tenant,project,auth,work,stream,ownership,&id,requested_session).await?;
        }
        data["execution"]["origin"] = json!({"session_id":e.get::<_,Option<String>>(1),
            "actor_id":e.get::<_,String>(2),"client_id":e.get::<_,Option<String>>(3),
            "actor_name":e.get::<_,Option<String>>(4),"basis":"recorded_execution_identity",
            "model":null,"model_basis":"not_recorded_at_execution"});
        if data["execution"]["receipt_details_available"] == false {
            data["missing"]["execution_receipt"] = json!("permission_restricted");
        }
    }
    // Latest participation is ordered by committed session events, not by
    // lease priority. Checkpoint revision is a fallback for imported sessions.
    data["last_participation"] = tx.query_opt("SELECT s.id,s.actor_id,a.display_name,s.client_id,s.state,
            GREATEST(ev.project_revision,c.observed_revision),
            (extract(epoch FROM COALESCE(ev.created_at,c.created_at))*1000)::bigint
        FROM awr_team.sessions s LEFT JOIN awr_team.actors a ON a.tenant_id=s.tenant_id AND a.id=s.actor_id
        LEFT JOIN awr_team.checkpoints c ON (c.tenant_id,c.project_id,c.id)=(s.tenant_id,s.project_id,s.latest_checkpoint_id)
        LEFT JOIN LATERAL (SELECT e.project_revision,e.created_at FROM awr_team.events e
          WHERE e.tenant_id=s.tenant_id AND e.project_id=s.project_id AND e.work_id=s.work_id
            AND e.workstream_id=s.workstream_id AND e.actor_id=s.actor_id AND e.payload_json->>'session_id'=s.id
          ORDER BY e.project_revision DESC,e.event_index DESC LIMIT 1) ev ON true
        WHERE s.tenant_id=$1 AND s.project_id=$2 AND s.scope_id='main' AND s.work_id=$3
          AND s.workstream_id=$4 AND s.ownership_version=$5 AND ($6::text IS NULL OR s.id=$6)
          AND (ev.project_revision IS NOT NULL OR c.observed_revision IS NOT NULL)
        ORDER BY GREATEST(ev.project_revision,c.observed_revision) DESC NULLS LAST,s.id DESC LIMIT 1",
        &[&tenant,&project,&work,&stream,&ownership,&requested_session]).await?
        .map(|r|json!({"session_id":r.get::<_,String>(0),"actor_id":r.get::<_,String>(1),
            "actor_name":r.get::<_,Option<String>>(2),"client_id":r.get::<_,String>(3),"state":r.get::<_,String>(4),
            "recorded_project_revision":r.get::<_,Option<i64>>(5).map(|v|v.to_string()),
            "at_unix_ms":r.get::<_,Option<i64>>(6),"basis":"latest_recorded_session_activity"})).unwrap_or(Value::Null);
    let deliveries = tx.query("SELECT id,pr_url,head_sha,gh_submitted,gh_approved,gh_merged,
        contract_hash,state,fact_source,observed_at,test_evidence_id FROM awr_team.pr_deliveries
        WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 ORDER BY created_at DESC,id DESC LIMIT 5",
        &[&tenant,&project,&work]).await?;
    data["pr_deliveries"] = json!(deliveries.iter().map(|r|json!({"id":r.get::<_,String>(0),
        "url":r.get::<_,String>(1),"head_sha":r.get::<_,String>(2),"submitted":r.get::<_,bool>(3),
        "approved":r.get::<_,bool>(4),"merged":r.get::<_,bool>(5),"contract_matches_current":r.get::<_,String>(6)==contract,
        "state":r.get::<_,String>(7),"fact_source":r.get::<_,String>(8),"observed_at":r.get::<_,String>(9),
        "test_evidence_id":r.get::<_,Option<String>>(10)})).collect::<Vec<_>>());
    data["last_activity_at_unix_ms"] = tx.query_one("SELECT (extract(epoch FROM max(created_at))*1000)::bigint
        FROM awr_team.events WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND workstream_id=$4",
        &[&tenant,&project,&work,&stream]).await?.get::<_,Option<i64>>(0).map_or(Value::Null,|v|json!(v));
    data["waiting_user"] = json!(
        tx.query_one(
            "SELECT EXISTS(SELECT 1 FROM awr_team.wait_items
        WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='open')",
            &[&tenant, &project, &work]
        )
        .await?
        .get::<_, bool>(0)
    );
    let owns_session = data["session"]["actor_id"] == auth.actor_id
        && data["session"]["client_id"] == auth.client_id;
    // Keep the added read futures on the heap. Preparation is nested inside
    // handoff/recovery commands; inlining optional advice into those futures
    // otherwise enlarges their caller's stack even when no fact is returned.
    data["collaboration"] = Box::pin(super::inbox::facts(
        tx, tenant, project, auth, work, &contract, ownership,
    ))
    .await?;
    Box::pin(super::inbox::observe_source(
        tx,
        tenant,
        project,
        auth,
        work,
        &mut data["collaboration"],
    ))
    .await?;
    if data["collaboration"]["source_state"]["observation"] != "not_measured" {
        data["collaboration"]["source_state"]["query_started_at_unix_ms"] = json!(observed);
    }
    let ready = Box::pin(crate::workstream_command::task_intake::dependencies_ready(
        tx, tenant, project, auth, work, stream,
    ))
    .await?;
    data["dependencies_ready_advisory"] = json!(ready);
    let stream_id = stream.parse().map_err(|_| PgError::SourceDivergence)?;
    let role_hint = super::inbox::condition(auth, &data, stream_id, work, ready);
    let hint = crate::workstream_command::task_intake::guidance(
        &data,
        super::guidance::with_collaboration(&data, true, owns_session, role_hint),
    );
    let hint = super::guidance::authorized(auth, stream_id, work, hint);
    let expected_authority_version = auth.catalog.get(stream_id)?.authority_version.to_string();
    let inputs =
        if hint["code"] == "prepare_execution" && hint["action"]["op"] == "execution.prepare" {
            selected_session_id
                .zip(selected_session_version)
                .zip(selected_claim)
                .zip(data["runtime"]["work_version"].as_str())
                .map(
                    |(
                        (
                            (session_id, expected_session_version),
                            (claim_id, expected_fence, expected_lease_version),
                        ),
                        expected_work_version,
                    )| {
                        super::guidance::NextCommandInputs::ExecutionPrepare(
                            super::guidance::NextExecutionPrepareInputs {
                                workstream_id: stream_id,
                                work_id: work.to_string(),
                                coordinator_epoch: auth.epoch.clone(),
                                expected_project_revision: auth.revision.to_string(),
                                expected_authority_version: expected_authority_version.clone(),
                                expected_ownership_version: ownership.to_string(),
                                expected_contract_hash: contract.clone(),
                                session_id,
                                expected_session_version,
                                claim_id,
                                expected_fence,
                                expected_lease_version,
                                expected_work_version: expected_work_version.to_string(),
                            },
                        )
                    },
                )
        } else if owns_session && hint["action"]["op"] == "session.checkpoint" {
            selected_session_id.zip(selected_session_version).map(
                |(session_id, expected_session_version)| {
                    super::guidance::NextCommandInputs::SessionCheckpoint(
                        super::guidance::NextSessionCheckpointInputs {
                            workstream_id: stream_id,
                            work_id: work.to_string(),
                            coordinator_epoch: auth.epoch.clone(),
                            expected_project_revision: auth.revision.to_string(),
                            expected_authority_version: expected_authority_version.clone(),
                            expected_ownership_version: ownership.to_string(),
                            expected_contract_hash: contract.clone(),
                            session_id,
                            expected_session_version,
                            require_client_info: hint["code"] == "declare_client",
                        },
                    )
                },
            )
        } else {
            None
        };
    data["guidance"] = hint;
    Ok((data, inputs))
}
