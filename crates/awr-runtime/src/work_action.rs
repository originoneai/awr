use crate::mutation::{ProposalReport, ReviewProposalAction, ReviewProposalRequest};
use awr_core::*;
use awr_source::verify_mutation_source;
use awr_store::Store;
use std::{collections::BTreeSet, path::Path};

#[derive(Debug, Clone)]
pub struct WorkActionRequest {
    pub target: String,
    pub session_id: Id,
    pub expected_revision: Revision,
    pub input: WorkActionInput,
}

/// Source checks accompany database readiness; an unindexed external dependency edit
/// must not permit an action based on an obsolete completed projection.
pub(crate) fn verify_work_dependencies(
    store: &Store,
    root: &Path,
    project: Id,
    patch: &MutationPatch,
) -> Result<()> {
    if patch.host_edit.as_ref().is_some_and(|h| {
        matches!(
            h.action,
            HostEditAction::ActivateDraft | HostEditAction::ConfirmOrdinary
        )
    }) {
        for source in store.sources(project)? {
            let (_, _, actual) = awr_source::inspect_registered_source(root, &source)?;
            if source.freshness != Freshness::Fresh || actual.fingerprint != source.fingerprint {
                return Err(Error::SourceConflict(
                    "draft activation requires current goal, rule, plan and dependency sources"
                        .into(),
                ));
            }
        }
    }
    if patch
        .host_edit
        .as_ref()
        .is_some_and(|h| h.action == HostEditAction::ConfirmOrdinary)
    {
        let receipt: OrdinaryCompletion =
            serde_json::from_value(patch.changes["ordinary_completion"].clone())?;
        for artifact in &receipt.artifacts {
            crate::read::read_registered_file(
                store,
                project,
                &artifact.locator,
                None,
                Some(&artifact.sha256),
                1024 * 1024,
            )?;
        }
    }
    if !patch
        .work_action
        .as_ref()
        .is_some_and(|b| b.action.needs_dependencies())
    {
        return Ok(());
    }
    verify_required_sources(
        store,
        root,
        project,
        &patch.target.meta.external_key,
        patch.target.meta.source_ref.source_id,
    )
}

pub(crate) fn verify_required_sources(
    store: &Store,
    root: &Path,
    project: Id,
    key: &str,
    source: Id,
) -> Result<()> {
    let graph = store.dependency_closure(project, key, true)?;
    let mut checked = BTreeSet::from([source]);
    for dependency in &graph.dependencies {
        if checked.insert(dependency.source.id) {
            let probe = MutationPatch {
                version: 1,
                target: MutationTarget {
                    kind: EntityKind::WorkItem,
                    meta: dependency.item.meta.clone(),
                },
                source_config: dependency.source.config.clone(),
                intent: "Verify the current required dependency source".into(),
                changes: serde_json::json!({"next_action":"verification only"}),
                host_edit: None,
                work_action: None,
            };
            verify_mutation_source(root, &dependency.source, &probe)?;
        }
    }
    if graph.edges.iter().any(|e| !checked.contains(&e.source.id)) {
        return Err(Error::SourceConflict(
            "dependency edge source has no verified current work projection".into(),
        ));
    }
    Ok(())
}

/// An explicit work action authorizes its deterministic proposal. Approval receipts
/// represent this caller request, never an independent human-review assertion.
pub fn perform_work_action(
    store: &mut Store,
    root: &Path,
    request: &WorkActionRequest,
) -> Result<ProposalReport> {
    let root = root.canonicalize()?;
    let project = store.project_by_root(&root)?;
    if project.project_revision != request.expected_revision {
        return Err(Error::RevisionConflict {
            expected: request.expected_revision,
            actual: project.project_revision,
        });
    }
    let target = store.mutation_target(project.id, EntityKind::WorkItem, &request.target)?;
    let work: WorkItem = serde_json::from_value(target.item)?;
    let (binding, changes) = request.input.plan(&work)?;
    let patch = MutationPatch {
        version: 1,
        target: MutationTarget {
            kind: EntityKind::WorkItem,
            meta: work.meta,
        },
        source_config: target.source.config.clone(),
        intent: request.input.reason.clone(),
        changes,
        host_edit: None,
        work_action: Some(binding),
    };
    verify_mutation_source(&root, &target.source, &patch)?;
    verify_work_dependencies(store, &root, project.id, &patch)?;
    let actor = store.session(project.id, request.session_id)?.agent_id;
    let (proposal, event) = store.create_proposal(
        project.id,
        request.expected_revision,
        MutationDraft {
            source_id: target.source.id,
            base_fingerprint: target.source.fingerprint,
            mutation_type: patch.mutation_type().into(),
            patch,
            created_by_session: Some(request.session_id),
        },
    )?;
    let mut report = apply_work_proposal(
        store,
        &root,
        proposal,
        event,
        actor,
        request.input.reason.clone(),
    )?;
    if report.failure.is_none() {
        report.close_out = crate::close_out::close_out(
            store,
            project.id,
            request.session_id,
            request.input.action,
        );
    }
    Ok(report)
}

pub(crate) fn apply_work_proposal(
    store: &mut Store,
    root: &Path,
    proposal: MutationProposal,
    event: Event,
    actor: String,
    reason: String,
) -> Result<ProposalReport> {
    let id = proposal.id;
    let renewal = crate::claim_policy::renewal_for(root);
    let mut result = crate::mutation::report(proposal, event, false, None, None);
    for (stage, action) in [
        ("submit", ReviewProposalAction::Submit),
        ("approve", ReviewProposalAction::Approve),
        ("apply", ReviewProposalAction::Apply),
    ] {
        result = crate::review_proposal_renewing(
            store,
            &root,
            &ReviewProposalRequest {
                proposal_id: id,
                expected_revision: result.project_revision,
                action,
                actor: actor.clone(),
                reason: reason.clone(),
            },
            renewal,
        )
        .map_err(|error| Error::WorkActionIncomplete {
            proposal_id: id,
            stage: stage.into(),
            reason: error.to_string(),
        })?;
        if result.failure.is_some() {
            break;
        }
    }
    Ok(result)
}
