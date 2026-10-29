use awr_core::*;
use awr_store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssessManagementRequest {
    pub work: String,
    pub branch: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManageWorkRequest {
    pub work: String,
    pub session: Id,
    pub expected_revision: Revision,
    pub request_key: String,
    pub contract_fingerprint: String,
    pub observation: ManagementObservation,
}

fn latest(
    store: &Store,
    project: Id,
    work: Id,
    branch: Option<Id>,
    kind: &str,
) -> Result<Option<Event>> {
    store.latest_work_event(project, work, branch, kind)
}

fn terminal_goal(status: &str) -> bool {
    matches!(
        status.to_ascii_lowercase().as_str(),
        "completed" | "done" | "cancelled" | "canceled" | "archived" | "retired"
    )
}

fn goal_binding(goal: &Projected<Goal>, required: Option<bool>) -> Value {
    json!({
        "id": goal.item.meta.id,
        "key": goal.item.meta.external_key,
        "title": goal.item.title,
        "status": goal.item.status,
        "summary": goal.item.summary,
        "success_criteria": goal.item.success_criteria,
        "required": required,
    })
}

pub fn assess_management(
    store: &Store,
    root: &Path,
    request: &AssessManagementRequest,
) -> Result<Value> {
    let project = store.project_by_root(root)?;
    let branch = match &request.branch {
        Some(b) => store.resolve_branch(project.id, b)?,
        None => project.current_branch_id,
    };
    assess(store, project.id, &request.work, branch, None, None)
}

fn assess(
    store: &Store,
    project: Id,
    key: &str,
    branch: Option<Id>,
    provided: Option<&ManagementObservation>,
    observer: Option<&str>,
) -> Result<Value> {
    let readiness = store.work_readiness(project, key, branch, now_millis()?)?;
    let work = &readiness.work.item;
    let mut dependency_links = store
        .work_dependency_links(project)?
        .into_iter()
        .filter(|e| e.from_key == key)
        .collect::<Vec<_>>();
    dependency_links
        .sort_by(|a, b| (&a.to_key, a.required, a.id).cmp(&(&b.to_key, b.required, b.id)));
    let dependencies = dependency_links
        .iter()
        .map(|e| json!({"key":e.to_key,"required":e.required}))
        .collect::<Vec<_>>();
    let mut goal_links = store
        .work_goal_links(project)?
        .into_iter()
        .filter(|e| e.from_key == key)
        .collect::<Vec<_>>();
    goal_links.sort_by(|a, b| (&a.to_key, a.required, a.id).cmp(&(&b.to_key, b.required, b.id)));
    let scoped_goal_keys = if store.workstreams_enabled(project)? {
        let binding = store.workstream_binding(project, work.meta.id)?;
        Some(
            store
                .workstream_catalog(project)?
                .get(binding.workstream_id)?
                .goal_keys
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>(),
        )
    } else {
        None
    };
    let mut goal_diagnostics = Vec::new();
    let inherited_goals = goal_links.is_empty();
    let (goal_selection, mut goals) = if inherited_goals {
        let mut goals = Vec::new();
        if let Some(keys) = &scoped_goal_keys {
            for key in keys {
                match store.goal(project, key) {
                    Ok(goal) if !terminal_goal(&goal.item.status) => {
                        if goal.source.freshness != Freshness::Fresh {
                            goal_diagnostics.push("goal_context_not_fresh");
                        }
                        goals.push(goal_binding(&goal, None));
                    }
                    Ok(_) => {}
                    Err(Error::NotFound(_)) => {
                        goal_diagnostics.push("goal_context_unavailable");
                    }
                    Err(error) => return Err(error),
                }
            }
            ("workstream_goal_references", goals)
        } else {
            for goal in store.goals(project)?.into_iter().filter(|goal| {
                goal.source.role == "primary"
                    && matches!(goal.source.domain.as_str(), "goal" | "ledger")
                    && !terminal_goal(&goal.item.status)
            }) {
                if goal.source.freshness != Freshness::Fresh {
                    goal_diagnostics.push("goal_context_not_fresh");
                }
                goals.push(goal_binding(&goal, None));
            }
            ("project_primary_goal_sources", goals)
        }
    } else {
        let mut goals = Vec::new();
        for link in &goal_links {
            let scoped_unavailable = scoped_goal_keys
                .as_ref()
                .is_some_and(|keys| !keys.contains(&link.to_key));
            if scoped_unavailable {
                goal_diagnostics.push("linked_goal_unavailable");
                goals.push(json!({"id":null,"key":link.to_key,"required":link.required,
                    "title":null,"status":null,"summary":null,"success_criteria":null}));
                continue;
            }
            match store.goal(project, &link.to_key) {
                Ok(goal) => {
                    if goal.source.freshness != Freshness::Fresh {
                        goal_diagnostics.push("linked_goal_source_not_fresh");
                    }
                    goals.push(goal_binding(&goal, Some(link.required)));
                }
                Err(Error::NotFound(_)) => {
                    goal_diagnostics.push(if scoped_goal_keys.is_some() {
                        "linked_goal_unavailable"
                    } else {
                        "linked_goal_not_found"
                    });
                    goals.push(json!({"id":null,"key":link.to_key,"required":link.required,
                        "title":null,"status":null,"summary":null,"success_criteria":null}));
                }
                Err(error) => return Err(error),
            }
        }
        ("work_goal_links", goals)
    };
    goals.sort_by(|a, b| {
        (a["key"].as_str(), a["required"].as_bool())
            .cmp(&(b["key"].as_str(), b["required"].as_bool()))
    });
    goal_diagnostics.sort_unstable();
    goal_diagnostics.dedup();
    let legacy_goals = goal_links
        .iter()
        .map(|edge| json!({"key":edge.to_key,"required":edge.required}))
        .collect::<Vec<_>>();
    let mut legacy_contract = json!({"id":work.meta.id,
        "title":work.title,"summary":work.summary,"kind":work.kind,"acceptance":work.acceptance,
        "paths":work.paths,"tags":work.tags,"goals":legacy_goals,"dependencies":dependencies,
        "ordinary_work_policy":readiness.work.source.config["adapter_options"]["ordinary_work_policy"]});
    if !work.spec_refs.is_empty() {
        legacy_contract["spec_refs"] = json!(work.spec_refs);
    }
    let legacy_contract_fingerprint =
        awr_source::fingerprint(&serde_json::to_vec(&legacy_contract)?);
    // Progress, next_action, evidence additions and unrelated ledger edits do not invalidate scope.
    let mut contract = json!({"id":work.meta.id,
        "title":work.title,"summary":work.summary,"kind":work.kind,"acceptance":work.acceptance,
        "paths":work.paths,"tags":work.tags,
        "goals":{"selection":goal_selection,"bindings":goals},"dependencies":dependencies,
        "ordinary_work_policy":readiness.work.source.config["adapter_options"]["ordinary_work_policy"]});
    if !work.spec_refs.is_empty() {
        contract["spec_refs"] = json!(work.spec_refs);
    }
    if let Some(definition) = &work.definition {
        contract["definition"] = json!(definition);
    }
    let contract_fingerprint = awr_source::fingerprint(&serde_json::to_vec(&contract)?);
    let previous = latest(store, project, work.meta.id, branch, "management.assessed")?;
    let prior_continuous = previous
        .as_ref()
        .is_some_and(|e| e.payload["assessment"]["decision"]["mode"] == "continuous");
    let matching = previous
        .as_ref()
        .filter(|e| e.payload["assessment"]["contract_fingerprint"] == contract_fingerprint);
    let legacy_binding_upgrade = previous.as_ref().is_some_and(|event| {
        event.payload["assessment"]["contract_fingerprint"] == legacy_contract_fingerprint
            && legacy_contract_fingerprint != contract_fingerprint
    });
    let stored: Option<ManagementObservation> = matching
        .map(|e| serde_json::from_value(e.payload["input"]["observation"].clone()))
        .transpose()
        .map_err(|_| Error::Storage("invalid management observation receipt".into()))?;
    let observation = provided.or(stored.as_ref());
    let mut reasons = vec![];
    let mut reason = |code: &str, basis: &str, reference: String| {
        reasons.push(ManagementReason {
            code: code.into(),
            basis: basis.into(),
            reference,
        })
    };
    if !readiness.dependencies.missing_keys.is_empty()
        || !readiness.dependencies.cycle_keys.is_empty()
        || readiness
            .dependencies
            .dependencies
            .iter()
            .any(|d| d.item.status != WorkStatus::Completed)
    {
        reason("unresolved_dependencies", "source_projection", key.into());
    }
    if previous.is_some() && matching.is_none() && !legacy_binding_upgrade {
        reason(
            "work_contract_changed",
            "source_projection",
            contract_fingerprint.clone(),
        );
    }
    for (kind, code) in [
        ("mcp.wait_created", "persistent_wait"),
        ("work.handoff", "work_handoff"),
        ("session.resumed", "cross_session_resume"),
    ] {
        if let Some(event) = latest(store, project, work.meta.id, branch, kind)? {
            reason(code, "runtime_history", event.id.to_string());
        }
    }
    let executions = store.executions(project, Some(work.meta.id))?;
    for execution in executions
        .iter()
        .filter(|e| e.branch_id == branch && !e.state.terminal() && e.started_at.is_some())
    {
        reason(
            "execution_result_requires_query",
            "runtime_recorded_state",
            execution.id.to_string(),
        );
    }
    let decision = decide_management(observation, reasons, prior_continuous);
    let mut admission_gaps = vec![];
    if validate_criteria(&work.acceptance).is_err() {
        admission_gaps.push("source_acceptance_missing_or_ambiguous");
    }
    admission_gaps.extend(goal_diagnostics);
    if inherited_goals && goals.is_empty() {
        admission_gaps.push("goal_context_missing");
    }
    let record_required = previous.as_ref().is_none_or(|e| {
        e.payload["assessment"]["decision"]["mode"] != serde_json::to_value(decision.mode).unwrap()
    }) || matching.is_none();
    Ok(
        json!({"version":1,"work":key,"work_id":work.meta.id,"work_revision":work.meta.revision,
        "source_ref":work.meta.source_ref,"branch_id":branch,"contract_fingerprint":contract_fingerprint,
        "decision":decision,"observation":observation,"observation_basis":"host_assertion_not_independently_verified",
        "observer":observer.map(String::from).or_else(||matching.and_then(|e|e.payload["observer"].as_str().map(String::from))),
        "observation_event":matching.map(|e|e.id),"previous_record":previous.as_ref().map(|e|e.id),
        "goal_binding_upgrade_required":legacy_binding_upgrade,
        "record_required":record_required,"admission_gaps":admission_gaps,
        "next_action":if !admission_gaps.is_empty(){"repair_required_context_before_execution"}else if record_required{"record_current_assessment_with_explicit_host_observations"}else{"follow_required_actions_and_existing_workflow"},
        "scope":"Management requirements only. Readiness, required goals/rules, authorization and completion remain enforced by their existing operations."}),
    )
}

pub fn manage_work(store: &mut Store, root: &Path, request: &ManageWorkRequest) -> Result<Value> {
    ensure_public_data(request)?;
    request.observation.validate(now_millis()?)?;
    if request.request_key.trim().is_empty()
        || request.request_key.len() > 512
        || request.request_key.chars().any(char::is_control)
    {
        return Err(Error::InvalidInput(
            "management requires a bounded stable request_key".into(),
        ));
    }
    let project = store.project_by_root(root)?;
    let work = store.work_item(project.id, &request.work)?;
    let input = json!({"work":request.work,"session":request.session,"contract_fingerprint":request.contract_fingerprint,"observation":request.observation});
    if let Some(event) =
        store.management_by_key(project.id, work.item.meta.id, &request.request_key)?
    {
        if event.payload["input"] != input {
            return Err(Error::SourceConflict(
                "management request_key already binds different observations".into(),
            ));
        }
        return Ok(
            json!({"ok":true,"already_recorded":true,"event":event,"assessment":event.payload["assessment"],"project_revision":project.project_revision}),
        );
    }
    let manifest = awr_source::Manifest::load(root)?;
    if !awr_source::index_project(store, root, &manifest, false)?.ok {
        return Err(Error::SourceStale(
            "refresh sources before recording management".into(),
        ));
    }
    let actual = store.project(project.id)?.project_revision;
    if actual != request.expected_revision {
        return Err(Error::RevisionConflict {
            expected: request.expected_revision,
            actual,
        });
    }
    let session = store.session(project.id, request.session)?;
    if session.work_item_id != Some(work.item.meta.id) {
        return Err(Error::RuleViolation(
            "management session must belong to the selected work".into(),
        ));
    }
    let mut assessment = assess(
        store,
        project.id,
        &request.work,
        session.branch_id,
        Some(&request.observation),
        Some(&session.agent_id),
    )?;
    if assessment["contract_fingerprint"] != request.contract_fingerprint {
        return Err(Error::SourceConflict(
            "work contract changed; read current assessment before recording observations".into(),
        ));
    }
    assessment["record_required"] = json!(false);
    assessment["goal_binding_upgrade_required"] = json!(false);
    if assessment["admission_gaps"]
        .as_array()
        .is_some_and(|a| a.is_empty())
    {
        assessment["next_action"] = json!("follow_required_actions_and_existing_workflow");
    }
    let event=store.record_management(project.id,request.expected_revision,session.id,work.item.meta.id,
        json!({"version":1,"request_key":request.request_key,"input":input,"observer":session.agent_id,"assessment":assessment}))?;
    Ok(
        json!({"ok":true,"already_recorded":false,"assessment":assessment,"project_revision":event.project_revision,"event":event,
        "source_write_performed":false,"completion_claimed":false}),
    )
}
