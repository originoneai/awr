use crate::fs_sync::sync_directory;
use crate::mutation::{ProposalReport, ReviewProposalRequest, report};
use awr_core::*;
use awr_source::{
    Locator, fingerprint, inspect_mutation_source, open_dir_exact, open_file_exact,
    parse_mutation_projection, prepare_yaml_mutation, verify_mutation_source,
};
use awr_store::Store;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use std::{
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::{Read, Write},
    path::Path,
};

pub(crate) fn directory(parent: &Dir, name: &str, path: &Path) -> Result<Dir> {
    match parent.create_dir(name) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e.into()),
    }
    let meta = parent.symlink_metadata(name)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(Error::RuleViolation(format!(
            "{}: mutation recovery directory must be a real directory",
            path.display()
        )));
    }
    Ok(parent.open_dir_nofollow(name)?)
}
pub(crate) fn recovery_root(root: &Path) -> Result<Dir> {
    let runtime = root.join(".awr");
    let project = open_dir_exact(root)?;
    let meta = project.symlink_metadata(".awr")?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(Error::RuleViolation(format!(
            "{}: runtime directory must not be a symlink",
            runtime.display()
        )));
    }
    let runtime_dir = project
        .open_dir_nofollow(".awr")
        .map_err(|e| Error::SourceUnavailable(format!("{}: {e}", runtime.display())))?;
    directory(&runtime_dir, "mutations", &runtime.join("mutations"))
}
pub(crate) struct SourceLock(File);
impl Drop for SourceLock {
    fn drop(&mut self) {
        // Release this writer's reservation explicitly. A concurrent process spawn
        // can briefly retain the same file description until close-on-exec runs;
        // dropping our descriptor alone need not release the OS lock at this point.
        let _ = self.0.unlock();
    }
}
pub(crate) fn source_lock(root: &Path, source: Id) -> Result<SourceLock> {
    named_lock(root, &format!("{source}.lock"))
}
pub(crate) fn named_lock(root: &Path, name: &str) -> Result<SourceLock> {
    named_lock_for_owner(root, name, None)
}
pub(crate) fn named_lock_for_owner(
    root: &Path,
    name: &str,
    owner: Option<&str>,
) -> Result<SourceLock> {
    if name.len() > 100
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
    {
        return Err(Error::InvalidInput("invalid mutation lock name".into()));
    }
    let directory = recovery_root(root)?;
    let path = root.join(".awr/mutations").join(&name);
    if directory
        .symlink_metadata(&name)
        .is_ok_and(|m| m.file_type().is_symlink() || !m.is_file())
    {
        return Err(Error::RuleViolation(format!(
            "{}: source lock must be a regular file",
            path.display()
        )));
    }
    let mut options = OpenOptions::new();
    options
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .follow(FollowSymlinks::No);
    let file = directory
        .open_with(&name, &options)
        .map_err(|e| Error::SourceUnavailable(format!("{}: {e}", path.display())))?
        .into_std();
    if !file.metadata()?.is_file() {
        return Err(Error::RuleViolation(format!(
            "{}: source lock must be a regular file",
            path.display()
        )));
    }
    file.try_lock().map_err(|e| {
        Error::MutationConflict(format!(
            "{}: another writer holds the source lock: {e}",
            path.display()
        ))
    })?;
    let lock = SourceLock(file);
    let intent = path.with_extension("intent");
    match std::fs::symlink_metadata(&intent) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.into()),
        Ok(_) => {
            let mut bytes = Vec::new();
            open_file_exact(&intent)?
                .take(513)
                .read_to_end(&mut bytes)?;
            if bytes.len() > 512 || owner.map(str::as_bytes) != Some(bytes.as_slice()) {
                return Err(Error::MutationConflict("source has a pending batch; inspect its receipt and explicitly recover it before another write".into()));
            }
        }
    }
    Ok(lock)
}
pub(crate) fn new_file(directory: &Dir, name: &OsStr) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(directory.open_with(name, &options)?.into_std())
}
fn write_file(mut file: File, bytes: &[u8], permissions: Option<fs::Permissions>) -> Result<()> {
    file.write_all(bytes)?;
    if let Some(permissions) = permissions {
        file.set_permissions(permissions)?;
    }
    file.sync_all()?;
    Ok(())
}
fn create_file(
    directory: &Dir,
    name: &OsStr,
    bytes: &[u8],
    permissions: Option<fs::Permissions>,
) -> Result<()> {
    write_file(new_file(directory, name)?, bytes, permissions)
}
fn stage(root: &Path, plan: &MutationWritePlan, before: &[u8], after: &[u8]) -> Result<()> {
    let parent = recovery_root(root)?;
    let name = plan.id.to_string();
    parent.create_dir(&name)?;
    let directory = parent.open_dir_nofollow(&name)?;
    create_file(&directory, OsStr::new("before.yaml"), before, None)?;
    create_file(&directory, OsStr::new("after.yaml"), after, None)?;
    create_file(
        &directory,
        OsStr::new("plan.json"),
        &serde_json::to_vec_pretty(plan)?,
        None,
    )?;
    sync_directory(&directory)?;
    sync_directory(&parent)
}
fn stored_after(root: &Path, plan: &MutationWritePlan) -> Result<Vec<u8>> {
    let path = root.join(plan.recovery_directory());
    let parent = recovery_root(root)?;
    let name = plan.id.to_string();
    let meta = parent.symlink_metadata(&name)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(Error::RuleViolation(format!(
            "{}: recovery directory was replaced",
            path.display()
        )));
    }
    let directory = parent.open_dir_nofollow(&name)?;
    let mut after = None;
    for (name, size, fp) in [
        ("before.yaml", plan.before_size, &plan.before_fingerprint),
        ("after.yaml", plan.after_size, &plan.after_fingerprint),
    ] {
        let meta = directory.symlink_metadata(name)?;
        if !meta.is_file() || meta.file_type().is_symlink() {
            return Err(Error::RuleViolation(format!(
                "{}: recovery snapshot must be a regular file",
                path.join(name).display()
            )));
        }
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        let file = directory
            .open_with(name, &options)
            .map_err(|e| Error::SourceUnavailable(format!("{}: {e}", path.join(name).display())))?;
        if !file.metadata()?.is_file() {
            return Err(Error::RuleViolation(format!(
                "{}: recovery snapshot must be a regular file",
                path.join(name).display()
            )));
        }
        let mut bytes = Vec::new();
        file.take(16 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(Error::InvalidInput(
                "recovery snapshot exceeds 16 MiB".into(),
            ));
        }
        if bytes.len() as u64 != size || fingerprint(&bytes) != *fp {
            return Err(Error::SourceConflict(
                "recovery snapshot differs from its immutable write plan".into(),
            ));
        }
        if name == "after.yaml" {
            after = Some(bytes);
        }
    }
    Ok(after.unwrap())
}

/// Hold one approved source directory for temp creation, replacement and cleanup.
/// Domain fingerprint/revision checks still run immediately before install.
pub(crate) struct SourceReplacement {
    directory: Dir,
    name: OsString,
    temp: Option<OsString>,
}
impl SourceReplacement {
    pub(crate) fn prepare(path: &Path, bytes: &[u8], permissions: fs::Permissions) -> Result<Self> {
        let directory = open_dir_exact(
            path.parent()
                .ok_or_else(|| Error::InvalidInput("source has no parent".into()))?,
        )?;
        let temp: OsString = format!(".awr-write-{}.tmp", Id::new()).into();
        // Ownership begins only after create_new succeeds; never remove a colliding file.
        let file = new_file(&directory, &temp)?;
        let replacement = Self {
            directory,
            name: path
                .file_name()
                .ok_or_else(|| Error::InvalidInput("source has no name".into()))?
                .to_owned(),
            temp: Some(temp),
        };
        write_file(file, bytes, Some(permissions))?;
        Ok(replacement)
    }
    pub(crate) fn install(&mut self) -> Result<()> {
        let temp = self.temp.as_ref().ok_or_else(|| {
            Error::InvalidTransition("source replacement was already installed".into())
        })?;
        self.directory.rename(temp, &self.directory, &self.name)?;
        self.temp = None;
        Ok(())
    }
    /// Publish a fully written new file without replacing a concurrent creator.
    pub(crate) fn install_new(&mut self) -> Result<()> {
        let temp = self
            .temp
            .as_ref()
            .ok_or_else(|| Error::InvalidTransition("source was already installed".into()))?;
        self.directory
            .hard_link(temp, &self.directory, &self.name)?;
        self.directory.remove_file(temp)?;
        self.temp = None;
        crate::fs_sync::sync_directory(&self.directory)
    }
    pub(crate) fn sync_parent(&self) -> Result<()> {
        crate::fs_sync::sync_directory(&self.directory)
    }
}
impl Drop for SourceReplacement {
    fn drop(&mut self) {
        // Cleanup uses the same held directory; it cannot follow a replacement parent.
        if let Some(temp) = &self.temp {
            let _ = self.directory.remove_file(temp);
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/security/paths/write_handles.rs"]
mod tests;
fn decorate(
    mut result: ProposalReport,
    store: &Store,
    project: Id,
    attempt: &MutationApplyAttempt,
    wrote: Option<bool>,
    outcome: &'static str,
) -> Result<ProposalReport> {
    result.project_revision = store.project(project)?.project_revision;
    result.source_write_performed = wrote;
    result.write_outcome = outcome;
    result.recovery_directory = Some(attempt.plan.recovery_directory());
    result.apply_attempt = store.proposal_apply_attempt(project, attempt.proposal_id)?;
    Ok(result)
}
fn pending(
    store: &Store,
    project: Id,
    attempt: &MutationApplyAttempt,
    wrote: Option<bool>,
    reason: String,
) -> Result<ProposalReport> {
    let failure = Error::MutationIncomplete {
        proposal_id: attempt.proposal_id,
        attempt_event_id: attempt.event_id,
        reason,
    };
    decorate(
        report(
            store.proposal(project, attempt.proposal_id)?,
            store.event(project, attempt.event_id)?,
            false,
            None,
            Some(failure),
        ),
        store,
        project,
        attempt,
        wrote,
        "pending_recovery",
    )
}
fn preparation_failure(
    store: &mut Store,
    project: Id,
    request: &ReviewProposalRequest,
    error: Error,
) -> Result<ProposalReport> {
    let (action, error) = match error {
        Error::MutationUnsupported(reason) => (
            ProposalAction::RequireManualApply,
            Error::ProposalRequired {
                proposal_id: request.proposal_id,
                reason,
            },
        ),
        e @ (Error::SourceConflict(_) | Error::SourceStale(_)) => (ProposalAction::Conflict, e),
        e => (ProposalAction::Fail, e),
    };
    let (proposal, event) = store.review_proposal(
        project,
        request.expected_revision,
        request.proposal_id,
        action,
        &request.actor,
        &format!("{}; {error}", request.reason),
    )?;
    Ok(report(proposal, event, false, None, Some(error)))
}
fn stopped(
    store: &mut Store,
    root: &Path,
    project: Id,
    request: &ReviewProposalRequest,
    attempt: &MutationApplyAttempt,
    wrote: Option<bool>,
    error: Error,
) -> Result<ProposalReport> {
    // A storage/finalization failure must retain the open attempt for exact recovery, even
    // when the source already has the planned bytes. Never conceal that fact with a bare error.
    let completion_written = store
        .proposal(project, attempt.proposal_id)
        .ok()
        .and_then(|proposal| proposal.bound_patch().ok())
        .filter(|patch| {
            patch
                .work_action
                .as_ref()
                .is_some_and(|b| b.action == WorkAction::Complete)
        })
        .is_some_and(|patch| {
            store
                .source(project, attempt.source_id)
                .is_ok_and(
                    |source| match inspect_mutation_source(root, &source, &patch) {
                        Ok((_, _, snapshot)) => {
                            snapshot.fingerprint == attempt.plan.after_fingerprint
                        }
                        Err(_) => {
                            wrote != Some(false)
                                || source.fingerprint == attempt.plan.after_fingerprint
                        }
                    },
                )
        });
    // Once an attempt is durable, an I/O/access failure cannot prove whether installation
    // happened in an earlier process. Keep both snapshots and retry the exact binding.
    // A later observed fingerprint/configuration conflict still stops without overwriting it.
    if matches!(
        error,
        Error::Storage(_)
            | Error::RevisionConflict { .. }
            | Error::Io(_)
            | Error::SourceUnavailable(_)
    ) || completion_written
    {
        return pending(store, project, attempt, wrote, error.to_string());
    }
    let conflict = matches!(error, Error::SourceConflict(_) | Error::SourceStale(_));
    if wrote != Some(false) {
        let source = store.source(project, attempt.source_id)?;
        if let Err(mark) = store.mark_source_freshness(&source, Freshness::Stale) {
            return pending(
                store,
                project,
                attempt,
                wrote,
                format!("{error}; cache freshness could not be updated: {mark}"),
            );
        }
    }
    let expected = store.project(project)?.project_revision;
    match store.fail_proposal_apply(
        project,
        expected,
        request.proposal_id,
        attempt.event_id,
        conflict,
        &request.actor,
        &format!("{}; {error}", request.reason),
    ) {
        Ok((proposal, event)) => decorate(
            report(proposal, event, false, None, Some(error)),
            store,
            project,
            attempt,
            wrote,
            "stopped",
        ),
        Err(failure) => pending(
            store,
            project,
            attempt,
            wrote,
            format!("{error}; could not record stop: {failure}"),
        ),
    }
}

pub(crate) fn apply(
    store: &mut Store,
    root: &Path,
    request: &ReviewProposalRequest,
    recover: bool,
) -> Result<ProposalReport> {
    apply_observed(store, root, request, recover, |_| Ok(()))
}

// The production entrypoint supplies only a no-op. Private tests can observe exact
// durability boundaries without adding environment switches to the shipped runtime.
fn apply_observed(
    store: &mut Store,
    root: &Path,
    request: &ReviewProposalRequest,
    recover: bool,
    mut observe: impl FnMut(&'static str) -> Result<()>,
) -> Result<ProposalReport> {
    if request.actor.trim().is_empty()
        || request.actor.len() > 256
        || request.reason.trim().is_empty()
        || request.reason.len() > 4096
    {
        return Err(Error::InvalidInput(
            "application requires an actor of 1..256 bytes and reason of 1..4096 bytes".into(),
        ));
    }
    let project = store.project_by_root(root)?.id;
    let proposal = store.proposal(project, request.proposal_id)?;
    ProposalAction::RequireManualApply.next_status(proposal.status)?;
    let _lock = source_lock(root, proposal.source_id)?;
    let existing = store
        .proposal_apply_attempt(project, proposal.id)?
        .filter(|attempt| attempt.resolved_event_id.is_none());
    if !recover {
        if let Some(attempt) = existing {
            return pending(
                store,
                project,
                &attempt,
                None,
                "use proposal recover to inspect and resume the recorded attempt".into(),
            );
        }
    }
    let patch = proposal.bound_patch()?;
    let source = store.source(project, proposal.source_id)?;
    let attempt = if recover {
        existing.ok_or_else(|| {
            Error::InvalidTransition("proposal has no unfinished application to recover".into())
        })?
    } else {
        let prepared = match (|| {
            verify_mutation_source(root, &source, &patch)?;
            crate::completion::verify_completion_proof(
                store,
                root,
                project,
                &patch,
                proposal.created_by_session,
            )?;
            crate::work_action::verify_work_dependencies(store, root, project, &patch)?;
            prepare_yaml_mutation(root, &source, &proposal, store.projection_ids(&source)?)
        })() {
            Ok(prepared) => prepared,
            Err(error) => return preparation_failure(store, project, request, error),
        };
        if open_file_exact(&prepared.path)?
            .metadata()?
            .permissions()
            .readonly()
        {
            return preparation_failure(
                store,
                project,
                request,
                Error::SourceUnavailable("source file is read-only".into()),
            );
        }
        stage(
            root,
            &prepared.plan,
            &prepared.before.bytes,
            &prepared.after.bytes,
        )?;
        observe("before_journal")?;
        store
            .begin_proposal_apply(
                project,
                request.expected_revision,
                proposal.id,
                prepared.plan,
                &request.actor,
                &request.reason,
            )?
            .0
    };
    observe("after_journal")?;
    let mut wrote = Some(false);
    let mut refreshed = false;
    let result = (|| {
        let after = stored_after(root, &attempt.plan)?;
        let source = store.source(project, proposal.source_id)?;
        let (locator, _, snapshot) = inspect_mutation_source(root, &source, &patch)?;
        let Locator::File(path) = locator else {
            return Err(Error::MutationUnsupported(
                "Git sources remain read-only".into(),
            ));
        };
        if awr_source::is_runtime_owned_path(root, &path) {
            return Err(Error::RuleViolation(
                "runtime-owned files cannot be rewritten; relocate authored documents outside .awr (except registered .awr/intake sources) and reindex".into(),
            ));
        }
        if snapshot.fingerprint == attempt.plan.before_fingerprint {
            verify_mutation_source(root, &source, &patch)?;
            let permissions = open_file_exact(&path)?.metadata()?.permissions();
            if permissions.readonly() {
                return Err(Error::SourceUnavailable("source file is read-only".into()));
            }
            let mut replacement = SourceReplacement::prepare(&path, &after, permissions)?;
            observe("after_temp")?;
            let before_replace = (|| {
                if patch.work_action.is_some() {
                    store.check_work_proposal(project, &proposal)?;
                    crate::completion::verify_completion_proof(
                        store,
                        root,
                        project,
                        &patch,
                        proposal.created_by_session,
                    )?;
                    crate::work_action::verify_work_dependencies(store, root, project, &patch)?;
                }
                let (current, _, observed) = inspect_mutation_source(root, &source, &patch)?;
                if !matches!(current,Locator::File(ref current) if current==&path)
                    || observed.fingerprint != attempt.plan.before_fingerprint
                {
                    return Err(Error::SourceConflict(
                        "source changed immediately before replacement".into(),
                    ));
                }
                // The current source must still refer to the original bytes at this last check.
                let actual = store.project(project)?.project_revision;
                let expected = if recover {
                    request.expected_revision
                } else {
                    attempt.project_revision
                };
                if actual != expected {
                    return Err(Error::RevisionConflict { expected, actual });
                }
                wrote = None;
                replacement.install()?;
                wrote = Some(true);
                observe("after_rename")?;
                sync_directory(&replacement.directory)?;
                observe("after_sync")
            })();
            before_replace?;
        } else if snapshot.fingerprint != attempt.plan.after_fingerprint {
            return Err(Error::SourceConflict("current source matches neither recorded snapshot; retained both snapshots without overwriting it".into()));
        }
        observe("before_projection")?;
        let source = store.source(project, proposal.source_id)?;
        let (_, spec, observed) = inspect_mutation_source(root, &source, &patch)?;
        if observed.fingerprint != attempt.plan.after_fingerprint {
            return Err(Error::SourceConflict(
                "source differs from the intended result after replacement".into(),
            ));
        }
        let (batch, hash) = parse_mutation_projection(
            &source,
            &spec,
            &observed,
            store.projection_ids(&source)?,
            &patch.target,
        )?;
        if hash != attempt.plan.target_after_hash {
            return Err(Error::SourceConflict(
                "reparsed target differs from the intended facts".into(),
            ));
        }
        let projected = store.commit_source_projection(&source, &observed.fingerprint, batch)?;
        observe("after_projection")?;
        refreshed = projected.revision != source.revision;
        let source = store.source(project, proposal.source_id)?;
        let (_, _, latest) = inspect_mutation_source(root, &source, &patch)?;
        if latest.fingerprint != attempt.plan.after_fingerprint {
            return Err(Error::SourceConflict(
                "source changed while its projection was rebuilt".into(),
            ));
        }
        crate::completion::verify_completion_proof(
            store,
            root,
            project,
            &patch,
            proposal.created_by_session,
        )?;
        let revision = store.project(project)?.project_revision;
        store.finish_proposal_apply(
            project,
            revision,
            proposal.id,
            attempt.event_id,
            &request.actor,
            &request.reason,
        )
    })();
    if result.is_ok() {
        observe("after_finalization")?;
    }
    let mut result = match result {
        Ok((proposal, event)) => decorate(
            report(proposal, event, false, None, None),
            store,
            project,
            &attempt,
            wrote,
            if recover { "recovered" } else { "applied" },
        ),
        Err(error) => stopped(store, root, project, request, &attempt, wrote, error),
    }?;
    result.source_refresh_performed = refreshed;
    Ok(result)
}

#[cfg(test)]
#[path = "../../../tests/recovery/mutations/engine.rs"]
mod recovery_tests;
