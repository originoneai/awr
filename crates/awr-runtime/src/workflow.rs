//! Small compositions of existing domain operations. Preparation never consumes context,
//! records evidence, starts execution, or asserts that verification ran.
use crate::work_definition_report;
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
    // Work without a declared definition pays nothing and gains no key.
    let definition = work
        .definition
        .is_some()
        .then(|| work_definition_report(store, project.id, &request.work))
        .transpose()?;
    let admission = store.admission_status(project.id, &request.work, branch)?;
    let admission_only = !readiness.ready
        && !readiness.diagnostics.is_empty()
        && readiness
            .diagnostics
            .iter()
            .all(|d| is_admission_code(&d.code));
    let reassessment_only = admission_only
        && readiness
            .diagnostics
            .iter()
            .all(|d| d.code.starts_with("reassessment_"));
    let suspended_only = admission_only
        && readiness
            .diagnostics
            .iter()
            .all(|d| d.code.ends_with("_suspended"));
    let mut prepared = json!({"version":1,"stage":"prepared","work":{
        "id":work.meta.id,"external_key":work.meta.external_key,"status":work.status,
        "revision":work.meta.revision,"source_ref":work.meta.source_ref,"next_action":work.next_action},
        "ready":readiness.ready,"diagnostics":readiness.diagnostics,"active_claims":readiness.active_claims,
        "context":context,"context_consumed":false,"completion_claimed":false,"management":management,
        "continuity":{"state":if waiting {"waiting_user"}else{"available"},"waits":waits},
        "next_action":if waiting {"resolve_persistent_wait_before_resuming"} else if !complete {"resolve_required_context_gaps"} else if reassessment_only {"record_reassessment_before_claiming"} else if suspended_only {"resolve_open_gaps_before_claiming"} else if admission_only {"resolve_admission_before_claiming"} else if !readiness.ready {"inspect_readiness_and_current_claim"} else {"consume_context_then_start_or_resume_session"}});
    if let Some(definition) = definition {
        prepared["definition"] = json!(definition);
    }
    // Work no policy governs gains no key, so existing output is unchanged.
    if admission.governed {
        prepared["admission"] = json!(admission);
    }
    if let Some(containment) = store.containment_block(project.id, &request.work, branch)? {
        prepared["containment"] = json!(containment);
    }
    if let Some(reassessment) = store.reassessment_block(project.id, &request.work, branch)? {
        prepared["reassessment"] = json!(reassessment);
    }
    if let Some(gaps) = store.gap_block(project.id, &request.work, branch)? {
        prepared["gaps"] = json!(gaps);
    }
    Ok(prepared)
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
    // Canonical report form is exactly one trailing LF line ending. The form
    // is advisory: common hosts write reports via json.dumps, JSON.stringify
    // or serde_json::to_writer, none of which append a newline, so the
    // preflight keeps accepting such reports and returns a descriptive
    // warning with the digest the canonical bytes would have. Line endings
    // are counted as LF or CRLF units, so Windows-formatted reports get the
    // same treatment (CRLF is non-canonical but never refused). The
    // preflight still never modifies the caller's report file.
    let mut content_end = bytes.len();
    let mut endings = 0usize;
    loop {
        if content_end >= 1 && bytes[content_end - 1] == b'\n' {
            content_end -= 1;
            if content_end >= 1 && bytes[content_end - 1] == b'\r' {
                content_end -= 1;
            }
            endings += 1;
        } else {
            break;
        }
    }
    let warning = if endings != 1 || bytes.ends_with(b"\r\n") {
        let mut canonical = bytes[..content_end].to_vec();
        canonical.push(b'\n');
        let would = format!("{:x}", Sha256::digest(&canonical));
        let shape = if endings == 0 {
            "it has no trailing newline"
        } else if endings > 1 {
            "it ends with more than one newline"
        } else {
            "it ends with a CRLF line ending instead of a single LF"
        };
        Some(format!(
            "report is not in canonical form: {shape}; the canonical form ends with exactly one trailing newline and its digest would be {would}"
        ))
    } else {
        None
    };
    let report = CompletionReport::parse(&bytes, &request.report).map_err(Error::InvalidSource)?;
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
    report
        .validate(
            &evidence,
            &request.work,
            &work.item.acceptance,
            now_millis()?,
        )
        .and_then(|()| report.validate_coverage(&evidence, &work.item.acceptance))
        .map_err(|error| CompletionReport::locate_error(&bytes, error))?;
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
        "warning":warning,
        "evidence":draft,"completion":completion,"verification_executed":false,
        "completion_claimed":false,"level_basis":"caller_assertion",
        "next_action":"record_evidence_then_complete_with_current_revision_and_owned_session"}),
    )
}
