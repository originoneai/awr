use awr_core::*;
use awr_store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::Path;

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
    let dependencies: Vec<_> = store
        .work_dependency_links(project)?
        .into_iter()
        .filter(|e| e.from_key == key)
        .map(|e| json!({"key":e.to_key,"required":e.required}))
        .collect();
    let goals: Vec<_> = store
        .work_goal_links(project)?
        .into_iter()
        .filter(|e| e.from_key == key)
        .map(|e| json!({"key":e.to_key,"required":e.required}))
        .collect();
    // Progress, next_action, evidence additions and unrelated ledger edits do not invalidate scope.
    let mut contract = json!({"id":work.meta.id,
        "title":work.title,"summary":work.summary,"kind":work.kind,"acceptance":work.acceptance,
        "paths":work.paths,"tags":work.tags,"goals":goals,"dependencies":dependencies,
        "ordinary_work_policy":readiness.work.source.config["adapter_options"]["ordinary_work_policy"]});
    if !work.spec_refs.is_empty() {
        contract["spec_refs"] = json!(work.spec_refs);
    }
    let contract_fingerprint = awr_source::fingerprint(&serde_json::to_vec(&contract)?);
    let previous = latest(store, project, work.meta.id, branch, "management.assessed")?;
    let prior_continuous = previous
        .as_ref()
        .is_some_and(|e| e.payload["assessment"]["decision"]["mode"] == "continuous");
    let matching = previous
        .as_ref()
        .filter(|e| e.payload["assessment"]["contract_fingerprint"] == contract_fingerprint);
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
    if previous.is_some() && matching.is_none() {
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
    if store.goals(project)?.is_empty() {
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
