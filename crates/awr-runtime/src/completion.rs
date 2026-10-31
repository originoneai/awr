use crate::{
    ProposalReport,
    read::read_registered_file,
    work_action::{apply_work_proposal, verify_required_sources},
};
use awr_core::*;
use awr_source::{
    Locator, inspect_mutation_source, read_yaml_mutation_record, verify_mutation_source,
};
use awr_store::Store;
use std::{collections::BTreeMap, path::Path};

#[derive(Debug, Clone)]
pub struct CompleteWorkRequest {
    pub target: String,
    pub session_id: Id,
    pub expected_revision: Revision,
    pub reason: String,
    pub input: CompletionInput,
    /// End the session in the same request once the work is completed; refused while the
    /// session holds claims on other work.
    pub end_session: bool,
}

/// Verify the exact stored proof and actual report bytes, including on journal recovery.
/// Supplied commands and levels are claims backed by reports, not commands to execute.
pub(crate) fn verify_completion_proof(
    store: &Store,
    root: &Path,
    project: Id,
    patch: &MutationPatch,
    session: Option<Id>,
) -> Result<()> {
    let Some(binding) = patch
        .work_action
        .as_ref()
        .and_then(|a| a.completion.as_ref())
    else {
        return Ok(());
    };
    let work = store
        .work_item(project, &patch.target.meta.external_key)?
        .item;
    if work.meta.id != patch.target.meta.id {
        return Err(Error::SourceConflict(
            "completion target identity changed".into(),
        ));
    }
    let session = store.session(
        project,
        session.ok_or_else(|| Error::InvalidInput("completion requires a session".into()))?,
    )?;
    store.check_completion_dependencies(project, &work.meta.external_key, session.branch_id)?;
    verify_required_sources(
        store,
        root,
        project,
        &work.meta.external_key,
        work.meta.source_ref.source_id,
    )?;
    store.check_completion_binding(project, &work, session.branch_id, binding)?;
    let source = store.source(project, work.meta.source_ref.source_id)?;
    let (target, _, _) = inspect_mutation_source(root, &source, patch)?;
    let mut reports = BTreeMap::new();
    for evidence in &binding.evidence {
        if evidence.locator.contains("://") {
            return Err(Error::Unsupported(
                "completion requires a registered local report file".into(),
            ));
        }
        let path = root.join(&evidence.locator).canonicalize()?;
        if matches!(&target,Locator::File(p) if p==&path)
            || (path.starts_with(root.join(".awr"))
                && !path.starts_with(root.join(".awr/artifacts")))
        {
            return Err(Error::EvidenceMissing(
                "completion report cannot be the changing source or mutable runtime state".into(),
            ));
        }
        if evidence.source_ref.is_some() {
            let target =
                store.mutation_target(project, EntityKind::Evidence, &evidence.external_key)?;
            let meta: ProjectionMeta = serde_json::from_value(target.item)?;
            if meta.id != evidence.id {
                return Err(Error::EvidenceMissing(
                    "evidence source identity changed".into(),
                ));
            }
            let probe = MutationPatch {
                version: 1,
                target: MutationTarget {
                    kind: EntityKind::Evidence,
                    meta,
                },
                source_config: target.source.config.clone(),
                intent: "Verify completion evidence source".into(),
                changes: serde_json::json!({"summary":"read only verification"}),
                host_edit: None,
                work_action: None,
            };
            verify_mutation_source(root, &target.source, &probe)?;
        }
        let bytes = read_registered_file(
            store,
            project,
            &evidence.locator,
            None,
            evidence.sha256.as_deref(),
            1024 * 1024,
        )?;
        let report = CompletionReport::parse(&bytes, &evidence.locator)
            .map_err(Error::EvidenceDiagnostic)?;
        report
            .validate(
                evidence,
                &work.meta.external_key,
                &work.acceptance,
                now_millis()?,
            )
            .map_err(|error| CompletionReport::locate_error(&bytes, error))?;
        reports.insert(evidence.id, report);
    }
    for mapping in &binding.acceptance {
        for id in &mapping.evidence {
            if !reports
                .get(id)
                .is_some_and(|r| r.covers(&mapping.criterion))
            {
                return Err(Error::EvidenceMissing(format!(
                    "evidence {id} does not verify acceptance: {}",
                    mapping.criterion
                )));
            }
        }
    }
    if work.blocker.as_ref().is_some_and(|b| !b.trim().is_empty()) {
        return Err(Error::DependencyBlocked(
            "work has an active blocker; resolve it before completion".into(),
        ));
    }
    // Only a blocking finding sends the work back; deferred records never hold completion.
    store.check_completion_findings(project, &work.meta.external_key, session.branch_id)?;
    Ok(())
}

/// Revision -> required dependencies -> acceptance -> evidence -> blocker -> preserving
/// proposal/write/reindex -> atomic completed receipt and claim release.
pub fn complete_work(
    store: &mut Store,
    root: &Path,
    request: &CompleteWorkRequest,
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
    let session = store.session(project.id, request.session_id)?;
    if request.end_session {
        crate::close_out::require_no_other_claims(store, project.id, session.id, work.meta.id)?;
    }
    let mut patch = MutationPatch {
        version: 1,
        target: MutationTarget {
            kind: EntityKind::WorkItem,
            meta: work.meta.clone(),
        },
        source_config: target.source.config.clone(),
        intent: request.reason.clone(),
        changes: serde_json::json!({"status":"completed"}),
        host_edit: None,
        work_action: None,
    };
    verify_mutation_source(&root, &target.source, &patch)?;
    store.check_completion_dependencies(project.id, &work.meta.external_key, session.branch_id)?;
    verify_required_sources(
        store,
        &root,
        project.id,
        &work.meta.external_key,
        target.source.id,
    )?;
    request.input.validate(&work)?;
    let mut selected = BTreeMap::new();
    for reference in request.input.references() {
        selected.insert(
            reference.clone(),
            store.evidence(project.id, &reference)?.item,
        );
    }
    let binding = CompletionBinding {
        version: 1,
        source_sha: request.input.source_sha.clone(),
        minimum_level: request.input.minimum_level,
        acceptance: request
            .input
            .acceptance
            .iter()
            .map(|a| AcceptanceEvidenceBinding {
                criterion: a.criterion.clone(),
                evidence: a.evidence.iter().map(|r| selected[r].id).collect(),
            })
            .collect(),
        evidence: selected
            .values()
            .map(|e| (e.id, e.clone()))
            .collect::<BTreeMap<_, _>>()
            .into_values()
            .collect(),
    };
    // Read and preserve the actual record; generic proposal application independently checks
    // these same changes so a manually built domain patch cannot discard source metadata.
    let record = read_yaml_mutation_record(&root, &target.source, &patch)?;
    patch.changes = completion_source_changes(&record, &binding)?;
    patch.work_action = Some(WorkActionBinding {
        action: WorkAction::Complete,
        from: work.status,
        to: WorkAction::Complete.next_status(work.status)?,
        completion: Some(binding),
    });
    patch.validate()?;
    verify_completion_proof(store, &root, project.id, &patch, Some(session.id))?;
    let (proposal, event) = store.create_proposal(
        project.id,
        request.expected_revision,
        MutationDraft {
            source_id: target.source.id,
            base_fingerprint: target.source.fingerprint,
            mutation_type: patch.mutation_type().into(),
            patch,
            created_by_session: Some(session.id),
        },
    )?;
    let mut report = apply_work_proposal(
        store,
        &root,
        proposal,
        event,
        session.agent_id,
        request.reason.clone(),
    )?;
    if report.failure.is_none() {
        if request.end_session {
            let receipt = crate::close_out::end_after_completion(
                store,
                project.id,
                session.id,
                report.event.id,
            );
            if receipt.status != "ended" {
                // The completion is durable; the request as a whole is a partial failure.
                let error = Error::InvalidTransition(
                    "the work is completed but its session could not be ended".into(),
                );
                report.ok = false;
                report.code = error.code();
                report.error = Some(error.report());
                report.failure = Some(error);
            }
            report.session_end = Some(receipt);
        } else {
            report.close_out =
                crate::close_out::close_out(store, project.id, session.id, WorkAction::Complete);
        }
    }
    Ok(report)
}
