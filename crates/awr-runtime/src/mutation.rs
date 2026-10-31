use awr_core::*;
use awr_source::{MutationSourceCheck, verify_mutation_source};
use awr_store::Store;
use serde::Serialize;
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct CreateProposalRequest {
    pub kind: EntityKind,
    pub target: String,
    pub intent: String,
    pub changes: Value,
    pub session_id: Option<Id>,
    pub expected_revision: Revision,
}
#[derive(Debug, Clone, Copy)]
pub enum ReviewProposalAction {
    Submit,
    Approve,
    Reject,
    Apply,
    Recover,
}
impl ReviewProposalAction {
    fn store_action(self) -> ProposalAction {
        match self {
            Self::Submit => ProposalAction::Submit,
            Self::Approve => ProposalAction::Approve,
            Self::Reject => ProposalAction::Reject,
            Self::Apply | Self::Recover => ProposalAction::RequireManualApply,
        }
    }
}
#[derive(Debug, Clone)]
pub struct ReviewProposalRequest {
    pub proposal_id: Id,
    pub expected_revision: Revision,
    pub action: ReviewProposalAction,
    pub actor: String,
    pub reason: String,
}
#[derive(Debug, Serialize)]
pub struct ProposalReport {
    pub ok: bool,
    pub code: &'static str,
    pub project_revision: Revision,
    pub proposal: MutationProposal,
    pub event: Event,
    pub source_refresh_performed: bool,
    pub source_write_performed: Option<bool>,
    pub write_outcome: &'static str,
    pub apply_attempt: Option<MutationApplyAttempt>,
    pub recovery_directory: Option<String>,
    pub source_check: Option<MutationSourceCheck>,
    pub error: Option<ErrorReport>,
    /// What to do with the session once the work is finished; only for a completion or a
    /// cancellation after which the session holds no other active claim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub close_out: Option<crate::CloseOut>,
    /// What ending the session did, when the completion asked for it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_end: Option<crate::SessionEndReceipt>,
    /// The CLI returns a nonzero exit status after printing the durable transition receipt.
    #[serde(skip)]
    pub failure: Option<Error>,
}
pub(crate) fn report(
    proposal: MutationProposal,
    event: Event,
    refreshed: bool,
    source_check: Option<MutationSourceCheck>,
    failure: Option<Error>,
) -> ProposalReport {
    ProposalReport {
        ok: failure.is_none(),
        code: failure
            .as_ref()
            .map(Error::code)
            .unwrap_or("proposal_recorded"),
        project_revision: event.project_revision,
        proposal,
        event,
        source_refresh_performed: refreshed,
        source_write_performed: Some(false),
        write_outcome: "not_requested",
        apply_attempt: None,
        recovery_directory: None,
        source_check,
        error: failure.as_ref().map(Error::report),
        close_out: None,
        session_end: None,
        failure,
    }
}
fn require_revision(store: &Store, project: Id, expected: Revision) -> Result<()> {
    let actual = store.project(project)?.project_revision;
    if actual != expected {
        return Err(Error::RevisionConflict { expected, actual });
    }
    Ok(())
}

/// Resolve an indexed target, verify current source bytes, and persist an immutable proposal.
/// Source drift requires an explicit reindex and a new revision; no hidden refresh precedes CAS.
pub fn create_proposal(
    store: &mut Store,
    root: &Path,
    request: &CreateProposalRequest,
) -> Result<ProposalReport> {
    let root = root.canonicalize()?;
    let project = store.project_by_root(&root)?.id;
    require_revision(store, project, request.expected_revision)?;
    let target = store.mutation_target(project, request.kind, &request.target)?;
    let patch = MutationPatch {
        version: 1,
        target: MutationTarget {
            kind: request.kind,
            meta: serde_json::from_value(target.item)?,
        },
        source_config: target.source.config.clone(),
        intent: request.intent.clone(),
        changes: request.changes.clone(),
        host_edit: None,
        work_action: None,
    };
    patch.validate()?;
    let checked = verify_mutation_source(&root, &target.source, &patch)?;
    let (proposal, event) = store.create_proposal(
        project,
        request.expected_revision,
        MutationDraft {
            source_id: target.source.id,
            base_fingerprint: target.source.fingerprint,
            mutation_type: "update_fields".into(),
            patch,
            created_by_session: request.session_id,
        },
    )?;
    Ok(report(proposal, event, false, Some(checked), None))
}

/// Approvals describe the bound snapshot, not permission to overwrite later file changes.
/// Conflict/failure transitions remain queryable even when their source is no longer readable.
pub fn review_proposal(
    store: &mut Store,
    root: &Path,
    request: &ReviewProposalRequest,
) -> Result<ProposalReport> {
    let root = root.canonicalize()?;
    let project = store.project_by_root(&root)?.id;
    require_revision(store, project, request.expected_revision)?;
    if matches!(
        request.action,
        ReviewProposalAction::Apply | ReviewProposalAction::Recover
    ) {
        return crate::mutation_apply::apply(
            store,
            &root,
            request,
            matches!(request.action, ReviewProposalAction::Recover),
        );
    }
    let proposal = store.proposal(project, request.proposal_id)?;
    let action = request.action.store_action();
    action.next_status(proposal.status)?;
    let mut checked = None;
    let mut failure = None;
    let mut chosen = action;
    if action != ProposalAction::Reject {
        let verification = (|| {
            let patch = proposal.bound_patch()?;
            let (source, active) = store.retained_source(project, proposal.source_id)?;
            if !active {
                return Err(Error::SourceConflict("proposal source is retired".into()));
            }
            let checked = verify_mutation_source(&root, &source, &patch)?;
            crate::completion::verify_completion_proof(
                store,
                &root,
                project,
                &patch,
                proposal.created_by_session,
            )?;
            Ok(checked)
        })();
        match verification {
            Ok(check) => checked = Some(check),
            Err(error) => {
                chosen = if matches!(
                    error,
                    Error::SourceConflict(_) | Error::SourceStale(_) | Error::MutationConflict(_)
                ) {
                    ProposalAction::Conflict
                } else {
                    ProposalAction::Fail
                };
                failure = Some(error);
            }
        }
    }
    let reason = match failure.as_ref() {
        Some(error) => format!("{}; {}", request.reason, error),
        None => request.reason.clone(),
    };
    // Reject empty caller reasons even when an observed failure would provide generated text.
    if request.reason.trim().is_empty() || request.reason.len() > 4096 {
        return Err(Error::InvalidInput(
            "proposal review needs a reason of 1..4096 bytes".into(),
        ));
    }
    let (proposal, event) = store.review_proposal(
        project,
        request.expected_revision,
        proposal.id,
        chosen,
        &request.actor,
        &reason,
    )?;
    Ok(report(proposal, event, false, checked, failure))
}
