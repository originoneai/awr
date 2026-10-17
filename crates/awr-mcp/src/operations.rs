use crate::{
    arguments::*,
    input_diagnostics::{invalid_input, parse_tool},
    project::{ReadProject, database, write_project},
};
use awr_context::{ContextRequest, DeltaBaseline, compile_branch_context, compile_context};
use awr_core::*;
use awr_runtime::{CompleteWorkRequest, Runtime, WorkActionRequest};
use awr_store::{SearchQuery, Store};
use rmcp::model::{CallToolResult, JsonObject};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub(crate) fn parse<T: DeserializeOwned>(args: Value) -> Result<T> {
    awr_core::ensure_public_value(&args)?;
    serde_json::from_value(args)
        .map_err(|_| Error::InvalidInput(crate::input_diagnostics::SCHEMA_MISMATCH.into()))
}
fn check_sha(sha: Option<&str>) -> Result<()> {
    if sha.is_some_and(|s| !is_source_sha(s)) {
        return Err(Error::InvalidInput(
            "source_sha must be a full source hash".into(),
        ));
    }
    Ok(())
}
fn branch(store: &Store, project: &Project, reference: Option<&str>) -> Result<Option<Id>> {
    match reference {
        Some(reference) => store.resolve_branch(project.id, reference),
        None => Ok(project.current_branch_id),
    }
}
fn short(text: &str) -> String {
    awr_core::public_summary(text, 240)
        .unwrap_or_else(|_| awr_core::SENSITIVE_CONTENT_WITHHELD.into())
}
fn brief(work: &Projected<WorkItem>) -> Value {
    let mut value = json!({"id":work.item.meta.id,"external_key":work.item.meta.external_key,"title":work.item.title,"source_ref":work.item.meta.source_ref,
        "archived":work.item.archived,"ordinary_completion":work.item.ordinary_completion,"ordinary_work_policy":work.source.config["adapter_options"]["ordinary_work_policy"],
        "status":work.item.status,"raw_status":work.item.raw_status,"summary":short(if work.item.summary.is_empty(){&work.item.title}else{&work.item.summary}),
        "priority":work.item.priority,"milestone":work.item.milestone,"owner":work.item.owner,"next_action":work.item.next_action,
        "blocker":work.item.blocker,"revision":work.item.meta.revision,"source_revision":work.item.meta.source_ref.source_revision,"freshness":work.source.freshness});
    if !work.item.spec_refs.is_empty() {
        value["spec_refs_count"] = json!(work.item.spec_refs.len());
        value["spec_refs_detail"] = json!({"work":work.item.meta.external_key,"cli_command":"work show","mcp_tool":"awr_work_get"});
    }
    value
}
fn ready_brief(work: &WorkReadiness) -> Value {
    let mut value = brief(&work.work);
    value["ready"] = json!(work.ready);
    value["diagnostics"] = json!(work.diagnostics);
    value["active_claims"] = json!(work.active_claims.iter().map(|c| json!({"id":c.id,"agent_id":c.agent_id,"session_id":c.session_id,"expires_at":c.expires_at})).collect::<Vec<_>>());
    value
}

pub(crate) fn is_read_only(name: &str) -> bool {
    matches!(
        name,
        "awr_project_status"
            | "awr_work_ready"
            | "awr_work_get"
            | "awr_work_prepare"
            | "awr_completion_prepare"
            | "awr_work_assess"
            | "awr_work_graph"
            | "awr_spec_check"
            | "awr_change_preview"
            | "awr_change_status"
            | "awr_context_compile"
            | "awr_search"
            | "awr_projects_list"
            | "awr_workstream"
            | "awr_team_handoff"
            | "awr_team_review"
            | "awr_session_get"
            | "awr_session_list"
            | "awr_operation_get"
            | "awr_compaction_get"
    )
}

pub(crate) fn call_as(
    root: &Path,
    name: &str,
    mut args: JsonObject,
    principal: Option<&str>,
) -> Result<CallToolResult> {
    if name == "awr_team_handoff" {
        return crate::team_handoff::handle(Value::Object(args));
    }
    if name == "awr_team_review" {
        return crate::team_review::handle(Value::Object(args));
    }
    let view = args.remove("response_view");
    let explain = match args.remove("explain") {
        None => false,
        Some(Value::Bool(v)) => v,
        Some(Value::Null) => false,
        Some(_) => {
            return invalid_input(
                "explain must be a boolean when provided",
                "/explain",
                "type",
            );
        }
    };
    if explain && !matches!(name, "awr_work_prepare" | "awr_work_assess") {
        return invalid_input(
            "explain is only supported on awr_work_prepare and awr_work_assess",
            "/explain",
            "unsupported",
        );
    }
    let supported = matches!(name, "awr_work_prepare" | "awr_work_transition");
    let action =
        view.as_ref().and_then(Value::as_str) == Some("action") && name == "awr_work_prepare";
    let summary = match view.as_ref() {
        None => false,
        Some(Value::String(view)) if supported && view == "full" => false,
        Some(Value::String(view)) if supported && view == "summary" => true,
        Some(Value::String(_)) if action => false,
        _ => {
            return invalid_input(
                "response_view requires full/summary on a work tool, or action on awr_work_prepare",
                "/response_view",
                "supported_view",
            );
        }
    };
    if summary && name == "awr_work_transition" && !args.contains_key("request_id") {
        return invalid_input(
            "summary transitions require request_id so the full receipt remains queryable",
            "/request_id",
            "required",
        );
    }
    let full_arguments = args.clone();
    if action {
        args.insert("response_view".into(), json!("action"));
    }
    // Presentation is outside the durable request identity and domain execution.
    let mut result = call_as_full(root, name, args, principal)?;
    // Summary/action stay success-only (legacy). Explain may attach on structured
    // error bodies too so callers can read the same assessment on incomplete prepare.
    let present = summary || action || explain;
    if present {
        if let Some(value) = result.structured_content.take() {
            let errored = result.is_error == Some(true);
            let mut value = if summary && !errored {
                awr_runtime::summarize_work_response(value)
            } else {
                value
            };
            if !errored && value.get("response_view").is_some() {
                value["response_view"]["full_result"] = if name == "awr_work_transition" {
                    json!({"tool":"awr_operation_get","arguments":{"request_id":full_arguments["request_id"]},"basis":"same client and project; recorded full result"})
                } else {
                    json!({"tool":name,"arguments":full_arguments,"basis":"fresh query; compare project_revision and context_hash"})
                };
            }
            if explain {
                // Attach only when the body looks like prepare/assess (has work key).
                if value.get("work").is_some() || value.get("decision").is_some() {
                    match awr_runtime::attach_assessment_explanation(
                        value.clone(),
                        &awr_runtime::AttachExplanationOptions {
                            enabled: true,
                            ..Default::default()
                        },
                    ) {
                        Ok(v) => value = v,
                        Err(_) if errored => {}
                        Err(e) => return Err(e),
                    }
                }
            }
            let displayed = if errored {
                CallToolResult::structured_error(value)
            } else {
                CallToolResult::structured(value)
            };
            result.content = displayed.content;
            result.structured_content = displayed.structured_content;
            result.is_error = displayed.is_error;
        }
    }
    Ok(result)
}

fn call_as_full(
    root: &Path,
    name: &str,
    mut args: JsonObject,
    principal: Option<&str>,
) -> Result<CallToolResult> {
    if serde_json::to_vec(&args)?.len() > 1024 * 1024 {
        return Err(Error::InvalidInput("tool arguments exceed 1 MiB".into()));
    }
    ensure_public_value(&Value::Object(args.clone()))?;
    let client = principal.unwrap_or("stdio");
    if crate::changes::NAMES.contains(&name)
        || matches!(name, "awr_operation_get" | "awr_operation_recover")
    {
        let input = Value::Object(args);
        return match name {
            "awr_operation_get" => crate::requests::inspect(root, input, client),
            "awr_operation_recover" => crate::requests::recover(root, input, client),
            _ => crate::changes::call(root, name, input, client),
        };
    }
    let request = match args.remove("request_id") {
        None => None,
        Some(Value::String(value)) => Some(value),
        Some(_) => {
            return invalid_input("request_id must be a string", "/request_id", "type");
        }
    };
    if request.is_some() && is_read_only(name) {
        return invalid_input(
            "read tools do not take request_id",
            "/request_id",
            "unsupported",
        );
    }
    if principal.is_some() && !is_read_only(name) && request.is_none() {
        return invalid_input(
            "shared writes require a stable request_id for outcome recovery",
            "/request_id",
            "required",
        );
    }
    let original = args.clone();
    if let Some(request) = &request {
        if let Some(result) = crate::requests::existing(root, client, request, name, &original)? {
            return Ok(result);
        }
    }
    if let Err(rejection) = crate::lifecycle::authorize(root, name, &mut args, principal) {
        return rejection.into_result();
    }
    if matches!(name, "awr_work_transition" | "awr_session_resume") {
        if let Some(session) = args.get("session").and_then(Value::as_str) {
            let store = Store::open_readonly(&database(root)?)?;
            let project = store.project_by_root(root)?;
            let session = match session.parse() {
                Ok(session) => session,
                Err(_) => {
                    return invalid_input("invalid session", "/session", "format");
                }
            };
            crate::waiting::require_resolved(&store, project.id, session)?;
        }
    }
    let apply = |args: JsonObject| {
        if crate::compaction::NAMES.contains(&name) {
            crate::compaction::call(root, name, Value::Object(args))
        } else if crate::lifecycle::NAMES.contains(&name) {
            crate::lifecycle::call(root, name, Value::Object(args), client)
        } else if matches!(
            name,
            "awr_session_wait" | "awr_session_reply" | "awr_source_reindex"
        ) {
            crate::waiting::call(root, name, Value::Object(args), client)
        } else {
            call(root, name, args)
        }
    };
    if let Some(request) = request {
        crate::requests::execute(root, client, &request, name, &original, args, apply)
    } else {
        apply(args)
    }
}

pub(crate) fn call(root: &Path, name: &str, mut args: JsonObject) -> Result<CallToolResult> {
    let action =
        name == "awr_work_prepare" && args.remove("response_view") == Some(json!("action"));
    let args = Value::Object(args);
    if serde_json::to_vec(&args)?.len() > 1024 * 1024 {
        return Err(Error::InvalidInput("tool arguments exceed 1 MiB".into()));
    }
    match name {
        "awr_work_transition" => return transition(root, parse_tool!(name, args)),
        "awr_event_append" => return append_event(root, parse_tool!(name, args)),
        "awr_evidence_record" => return evidence(root, parse_tool!(name, args)),
        "awr_work_manage" => {
            let mut store = Store::open_existing(&database(root)?)?;
            return Ok(CallToolResult::structured(awr_runtime::manage_work(
                &mut store,
                root,
                &parse_tool!(name, args),
            )?));
        }
        _ => (),
    }
    if name == "awr_project_status" {
        let status: StatusArgs = parse_tool!(name, args.clone());
        check_sha(status.source_sha.as_deref())?;
    }
    let mut view = match ReadProject::open(root) {
        Ok(view) => view,
        Err(error) if name == "awr_project_status" => {
            let initialized =
                root.join(".awr/project.toml").exists() || root.join(".awr/state.db").exists();
            let organization =
                awr_runtime::OrganizationReport::unavailable(initialized, &error.report().message);
            let value = json!({"ok":false,"read_only":true,"source_refresh_performed":false,"error":error.report(),"organization":organization,"total":null,"next_action":organization.next_action});
            ensure_public_value(&value)?;
            return Ok(CallToolResult::structured_error(value));
        }
        Err(error) => return Err(error),
    };
    let mut value = match name {
        "awr_project_status" => status(&view, parse_tool!(name, args))?,
        "awr_work_ready" => ready(&view, parse_tool!(name, args))?,
        "awr_work_get" => work(&view, parse_tool!(name, args))?,
        "awr_context_compile" => context(&mut view, root, parse_tool!(name, args))?,
        "awr_work_prepare" => {
            let request: awr_runtime::PrepareWorkRequest = parse_tool!(name, args);
            let prepared = awr_runtime::prepare_work(&mut view.store, root, &request)?;
            if action {
                awr_runtime::guide_prepared_work(&view.store, root, prepared, request.session)?
            } else {
                prepared
            }
        }
        "awr_completion_prepare" => {
            awr_runtime::prepare_completion(&view.store, root, &parse_tool!(name, args))?
        }
        "awr_work_assess" => {
            awr_runtime::assess_management(&view.store, root, &parse_tool!(name, args))?
        }
        "awr_work_graph" => awr_runtime::work_graph(&view.store, root, &parse_tool!(name, args))?,
        "awr_spec_check" => {
            awr_runtime::check_specifications(&view.store, root, &parse_tool!(name, args))?
        }
        "awr_search" => search(&mut view, parse_tool!(name, args))?,
        _ => return Err(Error::Unsupported(name.into())),
    };
    view.finish(root)?;
    let incomplete = (name == "awr_context_compile" && value["completeness"]["complete"] == false)
        || (name == "awr_work_prepare" && value["context"]["completeness"]["complete"] == false);
    for (key, item) in view.metadata().as_object().expect("metadata object") {
        // Context packs carry their own required gaps and source provenance.
        if name == "awr_context_compile"
            && matches!(key.as_str(), "source_issues" | "source_warnings")
        {
            continue;
        }
        value[key] = item.clone();
    }
    // Context renderings already passed per-chunk source proof checks inside the
    // refreshed context builder. Validate added fields separately; a second
    // context-free suspect scan would discard that proof.
    let mut checked = value.clone();
    if name == "awr_context_compile" {
        if let Some(context) = checked
            .get_mut("work_context")
            .and_then(Value::as_object_mut)
        {
            context.remove("rendered_context");
        }
    } else if name == "awr_work_prepare" {
        // This exact subtree comes from compile_context (or its branch variant),
        // before presentation-only guidance is added.
        checked.as_object_mut().unwrap().remove("context");
    }
    view.store.ensure_source_output(&checked)?;
    ensure_no_credentials_value(&value)?;
    if incomplete {
        value["ok"] = json!(false);
        value["error"] = json!(
            Error::ContextIncomplete(
                "L1 has required gaps; inspect completeness before execution".into()
            )
            .report()
        );
        Ok(CallToolResult::structured_error(value))
    } else {
        Ok(CallToolResult::structured(value))
    }
}

fn status(view: &ReadProject, args: StatusArgs) -> Result<Value> {
    let scope = awr_runtime::StatusScope {
        work: args.work,
        goal: args.goal,
        milestone: args.milestone,
    };
    let action_view = args.view.as_deref().unwrap_or("action") == "action";
    let summary = match args.view.as_deref().unwrap_or("action") {
        "full" if scope.is_empty() => false,
        "summary" | "action" => true,
        _ => {
            return Err(Error::InvalidInput(
                "view must be action, full or summary; scopes require action or summary".into(),
            ));
        }
    };
    let branch = branch(&view.store, &view.project, args.branch.as_deref())?;
    let works = view.store.work_items(view.project.id)?;
    let ready = view
        .store
        .ready_work(view.project.id, branch, now_millis()?)?;
    let organization = awr_runtime::inspect_organization(
        &view.store,
        &view.project,
        branch,
        args.source_sha.as_deref(),
        true,
        &works,
        &ready,
    )?;
    if summary {
        let project_status = if action_view {
            awr_runtime::action_status
        } else {
            awr_runtime::summarize_status
        };
        return project_status(
            &view.store,
            &view.project,
            &scope,
            &works,
            &ready,
            &organization,
        );
    }
    let mut counts = BTreeMap::<String, usize>::new();
    for work in &works {
        *counts
            .entry(
                serde_json::to_value(work.item.status)?
                    .as_str()
                    .unwrap()
                    .into(),
            )
            .or_default() += 1;
    }
    let mut current = works
        .iter()
        .filter(|w| matches!(w.item.status, WorkStatus::Claimed | WorkStatus::InProgress))
        .collect::<Vec<_>>();
    current.sort_by_key(|w| (&w.item.priority, &w.item.meta.external_key));
    let focus = current
        .iter()
        .find(|w| {
            organization
                .executable_work
                .contains(&w.item.meta.external_key)
        })
        .copied()
        .or_else(|| {
            ready
                .ready
                .iter()
                .find(|w| {
                    organization
                        .executable_work
                        .contains(&w.work.item.meta.external_key)
                })
                .map(|w| &w.work)
        });
    let work_next = focus
        .map(|w| w.item.next_action.clone())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| organization.next_action.clone());
    let next = if organization.business_execution_ready {
        work_next
    } else {
        organization.next_action.clone()
    };
    Ok(
        json!({"project":view.project.name,"project_id":view.project.id,"branch_id":branch,
        "counts":counts,"total":works.len(),"current":current.iter().take(5).map(|w| brief(w)).collect::<Vec<_>>(),
        "current_total":current.len(),"ready_count":ready.ready.len(),"blocked_count":ready.blocked.len(),
        "next_action":next,"suggested_work":focus.map(brief),"organization":organization}),
    )
}
fn ready(view: &ReadProject, args: ReadyArgs) -> Result<Value> {
    if args.limit == 0 || args.limit > 100 {
        return Err(Error::InvalidInput("ready limit must be 1..100".into()));
    }
    let branch = branch(&view.store, &view.project, args.branch.as_deref())?;
    let report = view
        .store
        .ready_work(view.project.id, branch, now_millis()?)?;
    let organization = awr_runtime::inspect_organization(
        &view.store,
        &view.project,
        branch,
        None,
        true,
        &view.store.work_items(view.project.id)?,
        &report,
    )?;
    let mut counts = BTreeMap::<String, usize>::new();
    for work in &report.blocked {
        for code in work
            .diagnostics
            .iter()
            .map(|d| d.code.clone())
            .collect::<BTreeSet<_>>()
        {
            *counts.entry(code).or_default() += 1;
        }
    }
    Ok(
        json!({"branch_id":branch,"ready":report.ready.iter().take(args.limit).map(ready_brief).collect::<Vec<_>>(),
        "ready_total":report.ready.len(),"truncated":report.ready.len()>args.limit,"blocked_total":report.blocked.len(),
        "queue_basis":"new claims only; blocked_total means not selectable, including active work; use status for continuation and waits",
        "diagnostic_counts":counts,"blocked_sample":report.blocked.iter().take(3).map(ready_brief).collect::<Vec<_>>(),"organization":organization}),
    )
}
fn work(view: &ReadProject, args: WorkArgs) -> Result<Value> {
    check_sha(args.source_sha.as_deref())?;
    let branch = branch(&view.store, &view.project, args.branch.as_deref())?;
    let work = view
        .store
        .work_readiness(view.project.id, &args.work, branch, now_millis()?)?;
    let decisions = view.store.decisions_for_work(view.project.id, &args.work)?;
    let evidence = view.store.evidence_for_work(
        view.project.id,
        &args.work,
        args.source_sha.as_deref(),
        branch,
    )?;
    let mut detail = ready_brief(&work);
    if !work.work.item.spec_refs.is_empty() {
        detail["spec_refs"] = json!(work.work.item.spec_refs);
    }
    Ok(
        json!({"work":detail,"acceptance":work.work.item.acceptance,"source_ref":work.work.item.meta.source_ref,
        "progress":awr_runtime::work_progress(&view.store,view.project.id,&work.work,branch)?,
        "required_dependencies":work.dependencies.dependencies.iter().map(|d| json!({"external_key":d.item.meta.external_key,"status":d.item.status,"revision":d.item.meta.revision,"source_revision":d.item.meta.source_ref.source_revision,"freshness":d.source.freshness})).collect::<Vec<_>>(),
        "missing_dependencies":work.dependencies.missing_keys,"dependency_cycles":work.dependencies.cycle_keys,
        "decisions":decisions.iter().map(|d| json!({"external_key":d.decision.item.meta.external_key,"summary":short(&d.decision.item.decision),"relevance":d.relevance,"reasons":d.reasons,"source_ref":d.decision.item.meta.source_ref})).collect::<Vec<_>>(),
        "evidence":evidence.iter().map(|e| json!({"external_key":e.evidence.item.external_key,"summary":short(&e.evidence.item.summary),"level":e.evidence.item.level,"currency":e.currency,"missing_bindings":e.missing_bindings,"locator":e.evidence.item.locator,"source_sha":e.evidence.item.source_sha,"reasons":e.reasons})).collect::<Vec<_>>(),
        "evidence_groups":awr_core::evidence_groups(&evidence),
        "evidence_currency_basis":{"requested_source_sha":args.source_sha,"branch_id":branch}}),
    )
}
fn context(view: &mut ReadProject, root: &Path, args: ContextArgs) -> Result<Value> {
    check_sha(args.source_sha.as_deref())?;
    if args.checkpoint.is_some() && args.after_revision.is_some() {
        return Err(Error::InvalidInput(
            "use checkpoint or after_revision, not both".into(),
        ));
    }
    let request = ContextRequest {
        work_item_key: args.work,
        session_id: args.session,
        detached: args.detached,
        agent_id: args.agent,
        branch_id: None,
        goal_keys: args.goals,
        paths: args.paths,
        tags: args.tags,
        source_sha: args.source_sha,
        intent: args.intent.unwrap_or_else(|| "work".into()),
        token_budget: args.budget.unwrap_or(5000),
        delta_baseline: if let Some(id) = args.checkpoint {
            DeltaBaseline::Checkpoint { id }
        } else if let Some(revision) = args.after_revision {
            DeltaBaseline::Revision { revision }
        } else {
            DeltaBaseline::Auto
        },
    };
    let report = match args.branch {
        Some(branch) => compile_branch_context(&mut view.store, root, &branch, &request)?,
        None => compile_context(&mut view.store, root, &request)?,
    };
    let session = report.session_id;
    let mut value = serde_json::to_value(report)?;
    if let Some(session) = session {
        let waits = view.store.mcp_waits(view.project.id, session)?;
        value["continuity"] = json!({"state":if waits.iter().any(|w|w.status=="waiting_user"){"waiting_user"}else{"available"},"waits":waits,"scope":"Runtime wait/reply records; separate from the bounded source context packet."});
    }
    Ok(value)
}
fn search(view: &mut ReadProject, args: SearchArgs) -> Result<Value> {
    let query = SearchQuery {
        text: args.text,
        kind: args
            .kind
            .map(|k| if k == "work" { "work_item".into() } else { k }),
        status: args.status,
        work_item_key: args.work,
        limit: args.limit,
    };
    let report = view.store.search(view.project.id, &query)?;
    Ok(
        json!({"hits":report.hits,"truncated":report.truncated,"index_policy_version":report.index_policy_version,"query":query}),
    )
}
fn transition(root: &Path, args: TransitionArgs) -> Result<CallToolResult> {
    let mut store = Store::open_existing(&database(root)?)?;
    let result = if args.action == WorkAction::Complete {
        if args.next_action.is_some() || args.summary.is_some() || args.blocker.is_some() {
            return Err(Error::InvalidInput(
                "complete uses only reason and completion bindings, not progress fields".into(),
            ));
        }
        let input = args.completion.ok_or_else(|| {
            Error::EvidenceMissing("complete requires an acceptance-to-evidence mapping".into())
        })?;
        if serde_json::to_vec(&input)?.len() > 64 * 1024 {
            return Err(Error::InvalidInput(
                "completion mapping exceeds 64 KiB".into(),
            ));
        }
        awr_runtime::complete_work(
            &mut store,
            root,
            &CompleteWorkRequest {
                target: args.work,
                session_id: args.session,
                expected_revision: args.expected_revision,
                reason: args.reason,
                input,
            },
        )?
    } else {
        if args.completion.is_some() {
            return Err(Error::InvalidInput(
                "only complete accepts completion bindings".into(),
            ));
        }
        awr_runtime::perform_work_action(
            &mut store,
            root,
            &WorkActionRequest {
                target: args.work,
                session_id: args.session,
                expected_revision: args.expected_revision,
                input: WorkActionInput {
                    action: args.action,
                    reason: args.reason,
                    next_action: args.next_action,
                    summary: args.summary,
                    blocker: args.blocker,
                },
            },
        )?
    };
    let mut value = serde_json::to_value(&result)?;
    if let Some(error) = result.failure {
        value["error"] = json!(error.report());
        Ok(CallToolResult::structured_error(value))
    } else {
        Ok(CallToolResult::structured(value))
    }
}
fn append_event(root: &Path, args: EventArgs) -> Result<CallToolResult> {
    let (mut store, project) = write_project(root, args.expected_revision)?;
    let branch = branch(&store, &project, args.branch.as_deref())?;
    let work_item_id = args
        .work
        .map(|w| store.work_item(project.id, &w).map(|w| w.item.meta.id))
        .transpose()?;
    let draft = EventDraft {
        work_item_id,
        session_id: args.session,
        branch_id: branch,
        event_type: args.event_type,
        importance: args.importance.unwrap_or_else(|| "normal".into()),
        summary: args.summary,
        payload: args.payload.unwrap_or_else(|| json!({})),
    };
    let mut runtime = Runtime::attach(&mut store, project.id)?;
    let invalid_payload_type = !draft.payload.is_object();
    let event = if args.branch.is_some() {
        runtime.append_event_in_branch(args.expected_revision, draft)
    } else {
        runtime.append_event(args.expected_revision, draft)
    };
    let event = match event {
        Err(Error::InvalidInput(message))
            if invalid_payload_type
                && message == "event requires a type, object payload and valid importance" =>
        {
            return invalid_input(message, "/payload", "type");
        }
        other => other?,
    };
    Ok(CallToolResult::structured(
        json!({"ok":true,"project_revision":event.project_revision,"event":event,"freshness_basis":"source_refresh","source_refresh_performed":true}),
    ))
}
fn evidence(root: &Path, args: EvidenceArgs) -> Result<CallToolResult> {
    let (mut store, project) = write_project(root, args.expected_revision)?;
    let branch_id = branch(&store, &project, args.branch.as_deref())?;
    let (evidence, event) = Runtime::attach(&mut store, project.id)?.record_evidence(
        args.expected_revision,
        EvidenceDraft {
            external_key: args.external_key,
            work_item_key: args.work,
            evidence_type: args.evidence_type,
            level: args.level,
            summary: args.summary,
            locator: args.locator,
            sha256: args.sha256,
            source_sha: args.source_sha,
            command: args.command,
            scope: args.scope,
            branch_id,
            verified_at: args.verified_at,
        },
    )?;
    Ok(CallToolResult::structured(
        json!({"ok":true,"project_revision":event.project_revision,"evidence":evidence,"event_id":event.id,"validation_basis":"caller_supplied_bindings","freshness_basis":"source_refresh","source_refresh_performed":true}),
    ))
}
