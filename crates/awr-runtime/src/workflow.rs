//! Small compositions of existing domain operations. Preparation never consumes context,
//! records evidence, starts execution, or asserts that verification ran.
use awr_context::{ContextRequest, compile_branch_context, compile_context};
use awr_core::*;
use awr_store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrepareWorkRequest {
    pub work: String,
    pub session: Option<Id>,
    pub branch: Option<String>,
    #[serde(default)]
    pub goals: Vec<String>,
    pub source_sha: Option<String>,
    pub budget: Option<usize>,
}

pub fn prepare_work(store: &mut Store, root: &Path, request: &PrepareWorkRequest) -> Result<Value> {
    ensure_public_data(request)?;
    let project = store.project_by_root(root)?;
    let ctx = ContextRequest {
        work_item_key: Some(request.work.clone()),
        session_id: request.session,
        detached: request.session.is_none(),
        goal_keys: request.goals.clone(),
        source_sha: request.source_sha.clone(),
        token_budget: request.budget.unwrap_or(5000),
        ..Default::default()
    };
    let context = match &request.branch {
        Some(branch) => compile_branch_context(store, root, branch, &ctx)?,
        None => compile_context(store, root, &ctx)?,
    };
    // Context selection is authoritative for the runtime branch. In a scoped
    // project an omitted branch means main, even if another workstream changed
    // the project's legacy branch default.
    let branch = context.completeness.branch_id;
    let readiness = store.work_readiness(project.id, &request.work, branch, now_millis()?)?;
    let complete = context.completeness.complete;
    let waits = request
        .session
        .map(|s| store.mcp_waits(project.id, s))
        .transpose()?
        .unwrap_or_default();
    let waiting = waits.iter().any(|w| w.status == "waiting_user");
    let work = &readiness.work.item;
    let management = crate::assess_management(
        store,
        root,
        &crate::AssessManagementRequest {
            work: request.work.clone(),
            branch: Some(
                branch
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "main".into()),
            ),
        },
    )?;
    Ok(json!({"version":1,"stage":"prepared","work":{
        "id":work.meta.id,"external_key":work.meta.external_key,"status":work.status,
        "revision":work.meta.revision,"source_ref":work.meta.source_ref,"next_action":work.next_action},
        "ready":readiness.ready,"diagnostics":readiness.diagnostics,"active_claims":readiness.active_claims,
        "context":context,"context_consumed":false,"completion_claimed":false,"management":management,
        "continuity":{"state":if waiting {"waiting_user"}else{"available"},"waits":waits},
        "next_action":if waiting {"resolve_persistent_wait_before_resuming"} else if !complete {"resolve_required_context_gaps"} else if !readiness.ready {"inspect_readiness_and_current_claim"} else {"consume_context_then_start_or_resume_session"}}))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrepareCompletionRequest {
    pub work: String,
    pub report: String,
    pub evidence_key: String,
    pub source_sha: String,
    pub level: EvidenceLevel,
    pub branch: Option<String>,
}

pub fn prepare_completion(
    store: &Store,
    root: &Path,
    request: &PrepareCompletionRequest,
) -> Result<Value> {
    ensure_public_data(request)?;
    if request.evidence_key.trim().is_empty()
        || request.evidence_key.len() > 4096
        || !is_source_sha(&request.source_sha)
        || verification_rank(request.level).is_none_or(|n| n < 2)
    {
        return Err(Error::InvalidInput("preflight requires a bounded evidence_key, full source_sha and at least locally_verified level declared by the caller".into()));
    }
    let project = store.project_by_root(root)?;
    let work = store.work_item(project.id, &request.work)?;
    validate_criteria(&work.item.acceptance)?;
    let branch = match &request.branch {
        Some(branch) => store.resolve_branch(project.id, branch)?,
        None => project.current_branch_id,
    };
    let bytes = crate::read::read_registered_file(
        store,
        project.id,
        &request.report,
        None,
        None,
        1024 * 1024,
    )?;
    let report: CompletionReport = serde_json::from_slice(&bytes).map_err(|e| {
        Error::InvalidSource(Box::new(SourceDiagnostic {message:"invalid completion report schema".into(), location:DiagnosticLocation {
            locator:Some(request.report.clone()), pointer:None, line:(e.line()>0).then_some(e.line()), column:(e.column()>0).then_some(e.column()),
        }, rule:"completion.report.v1".into(), repair:"Provide version, work_item, source_sha, command, scope, verified_at and checks with name, passed, details and criteria; see the completion report template.".into()}))
    })?;
    let evidence = Evidence {
        id: Id::new(),
        project_id: project.id,
        work_item_id: Some(work.item.meta.id),
        external_key: request.evidence_key.clone(),
        evidence_type: "verification_report".into(),
        level: request.level,
        summary: format!("Completion report for {}", request.work),
        locator: request.report.clone(),
        sha256: Some(format!("{:x}", Sha256::digest(&bytes))),
        source_sha: Some(request.source_sha.clone()),
        section_fingerprint: work.item.meta.source_ref.section_fingerprint.clone(),
        command: Some(report.command.clone()),
        scope: report.scope.clone(),
        source_ref: None,
        branch_id: branch,
        revision: 0,
        verified_at: Some(report.verified_at),
    };
    report.validate(
        &evidence,
        &request.work,
        &work.item.acceptance,
        now_millis()?,
    )?;
    if evidence.scope.is_empty() || !work.item.acceptance.iter().all(|c| report.covers(c)) {
        return Err(Error::EvidenceMissing(
            "report must cover every current acceptance criterion and declare a nonempty scope"
                .into(),
        ));
    }
    let completion = CompletionInput {
        version: 1,
        source_sha: request.source_sha.clone(),
        minimum_level: request.level,
        acceptance: work
            .item
            .acceptance
            .iter()
            .map(|c| AcceptanceEvidenceRequest {
                criterion: c.clone(),
                evidence: vec![request.evidence_key.clone()],
            })
            .collect(),
        required_evidence: vec![],
    };
    completion.validate(&work.item)?;
    let draft = json!({"external_key":evidence.external_key,"work":request.work,
        "evidence_type":evidence.evidence_type,"level":evidence.level,"summary":evidence.summary,
        "locator":evidence.locator,"sha256":evidence.sha256,"source_sha":evidence.source_sha,
        "command":evidence.command,"scope":evidence.scope,"branch":request.branch,
        "verified_at":evidence.verified_at});
    Ok(
        json!({"version":1,"stage":"report_validated","report_sha256":evidence.sha256,
        "evidence":draft,"completion":completion,"verification_executed":false,
        "completion_claimed":false,"level_basis":"caller_assertion",
        "next_action":"record_evidence_then_complete_with_current_revision_and_owned_session"}),
    )
}
