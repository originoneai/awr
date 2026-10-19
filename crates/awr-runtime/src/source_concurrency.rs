//! Recovery-log protected activation for precise and sharded source writes (WS-022).
//!
//! Extends existing SourceReplacement / mutation recovery journals. Stale whole-file
//! installs are refused. Supported shard candidates activate coherently; half-writes
//! and external edits remain recoverable without overwriting foreign bytes.
use crate::mutation_apply::{SourceReplacement, directory, named_lock, new_file, recovery_root};
use awr_core::*;
use awr_source::{
    Locator, Manifest, ShardCandidate, ShardObservation, ShardWrite, SourceWriteMode,
    document_path_registration, fingerprint, form_shard_candidate, index_project,
    inspect_registered_source, observe_candidate, observe_shard, open_file_exact,
    prepare_yaml_mutation, refuse_stale_whole_file, require_write_mode, source_write_mode,
};
use awr_store::Store;
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    ffi::OsStr,
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreciseReceipt {
    version: u32,
    phase: String,
    project_id: Id,
    project_revision: Revision,
    request_key: String,
    source_id: Id,
    path: PathBuf,
    before_fingerprint: String,
    after_fingerprint: String,
    adapter: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShardReceipt {
    version: u32,
    phase: String,
    project_id: Id,
    project_revision: Revision,
    request_key: String,
    candidate: ShardCandidate,
    applied: Vec<usize>,
}

/// Outcome of a protected precise-patch or shard activation.
#[derive(Debug)]
pub struct SourceConcurrencyReport {
    pub value: Value,
    pub failure: Option<Error>,
}

fn recovery_name(project: Id, key: &str) -> Result<String> {
    if key.trim().is_empty() || key.len() > 200 || key.chars().any(char::is_control) {
        return Err(Error::InvalidInput(
            "source concurrency request key must be a bounded printable string".into(),
        ));
    }
    let digest = fingerprint(format!("{project}:{key}").as_bytes());
    let short = digest.trim_start_matches("sha256:");
    Ok(format!("ws022-{}", &short[..32.min(short.len())]))
}

fn save_json(dir: &Dir, name: &str, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    let mut options = cap_std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    let mut file = dir.open_with(name, &options)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

fn load_json<T: for<'de> Deserialize<'de>>(dir: &Dir, name: &str) -> Result<Option<T>> {
    match dir.open(name) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(2 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
            if bytes.len() > 2 * 1024 * 1024 {
                return Err(Error::InvalidInput(
                    "source concurrency receipt exceeds 2 MiB".into(),
                ));
            }
            Ok(Some(serde_json::from_slice(&bytes)?))
        }
    }
}

/// Apply a reviewed precise-patch proposal under recovery-log + fingerprint protection.
pub fn activate_precise_patch(
    store: &mut Store,
    root: &Path,
    proposal: &MutationProposal,
    request_key: &str,
    expected_revision: Revision,
) -> Result<SourceConcurrencyReport> {
    let root = root.canonicalize()?;
    let source = store.source(proposal.project_id, proposal.source_id)?;
    require_write_mode(&source.adapter, SourceWriteMode::PrecisePatch)?;
    let existing = store.projection_ids(&source)?;
    let prepared = prepare_yaml_mutation(&root, &source, proposal, existing)?;
    refuse_stale_whole_file(&prepared.before.fingerprint, &prepared.before.fingerprint)?;
    refuse_stale_whole_file(&proposal.base_fingerprint, &prepared.before.fingerprint)?;

    let name = recovery_name(proposal.project_id, request_key)?;
    let parent = recovery_root(&root)?;
    let dir = directory(&parent, &name, &root.join(".awr/mutations").join(&name))?;
    let owner = name.clone();
    let _lock = named_lock(&root, &format!("{owner}.lock"))?;

    if let Some(existing) = load_json::<PreciseReceipt>(&dir, "receipt.json")? {
        if existing.phase == "completed"
            && existing.before_fingerprint == prepared.plan.before_fingerprint
            && existing.after_fingerprint == prepared.plan.after_fingerprint
        {
            return Ok(SourceConcurrencyReport {
                value: json!({
                    "ok": true,
                    "status": "completed",
                    "already_recorded": true,
                    "mode": "precise_patch",
                    "project_revision": existing.project_revision,
                    "source_id": existing.source_id,
                    "before_fingerprint": existing.before_fingerprint,
                    "after_fingerprint": existing.after_fingerprint,
                }),
                failure: None,
            });
        }
    }

    let mut receipt = PreciseReceipt {
        version: 1,
        phase: "planned".into(),
        project_id: proposal.project_id,
        project_revision: expected_revision,
        request_key: request_key.into(),
        source_id: source.id,
        path: prepared.path.clone(),
        before_fingerprint: prepared.plan.before_fingerprint.clone(),
        after_fingerprint: prepared.plan.after_fingerprint.clone(),
        adapter: source.adapter.clone(),
    };
    save_json(&dir, "receipt.json", &receipt)?;
    {
        let mut file = new_file(&dir, OsStr::new("before.bin"))?;
        file.write_all(&prepared.before.bytes)?;
        file.sync_all()?;
        let mut file = new_file(&dir, OsStr::new("after.bin"))?;
        file.write_all(&prepared.after.bytes)?;
        file.sync_all()?;
    }

    let mut wrote = false;
    let result = (|| -> Result<Revision> {
        let actual = store.project(proposal.project_id)?.project_revision;
        if actual != expected_revision {
            return Err(Error::RevisionConflict {
                expected: expected_revision,
                actual,
            });
        }
        let live = awr_source::read_capped(
            &prepared.path,
            awr_source::source_read_cap(&source.adapter)?,
        )?;
        let live_fp = fingerprint(&live);
        // Stale whole-file: refuse rather than overwrite another writer's change.
        refuse_stale_whole_file(&prepared.plan.before_fingerprint, &live_fp)?;
        let permissions = open_file_exact(&prepared.path)?.metadata()?.permissions();
        if permissions.readonly() {
            return Err(Error::RuleViolation(
                "precise patch destination is read-only".into(),
            ));
        }
        let mut replacement =
            SourceReplacement::prepare(&prepared.path, &prepared.after.bytes, permissions)?;
        let again = fingerprint(&awr_source::read_capped(
            &prepared.path,
            awr_source::source_read_cap(&source.adapter)?,
        )?);
        refuse_stale_whole_file(&prepared.plan.before_fingerprint, &again)?;
        replacement.install()?;
        replacement.sync_parent()?;
        wrote = true;
        receipt.phase = "applied".into();
        save_json(&dir, "receipt.json", &receipt)?;
        let indexed = index_project(store, &root, &Manifest::load(&root)?, false)?;
        if !indexed.ok {
            return Err(Error::SourceStale(
                "precise patch was written but indexing requires recovery".into(),
            ));
        }
        let projected = store.source(proposal.project_id, source.id)?;
        if projected.fingerprint != prepared.plan.after_fingerprint {
            return Err(Error::SourceConflict(
                "precise patch projection differs from reviewed after fingerprint".into(),
            ));
        }
        receipt.phase = "completed".into();
        receipt.project_revision = indexed.project_revision;
        save_json(&dir, "receipt.json", &receipt)?;
        Ok(indexed.project_revision)
    })();

    match result {
        Ok(revision) => Ok(SourceConcurrencyReport {
            value: json!({
                "ok": true,
                "status": "completed",
                "mode": "precise_patch",
                "already_recorded": false,
                "source_write_performed": wrote,
                "project_revision": revision,
                "source_id": source.id,
                "before_fingerprint": prepared.plan.before_fingerprint,
                "after_fingerprint": prepared.plan.after_fingerprint,
                "recovery_directory": format!(".awr/mutations/{owner}"),
            }),
            failure: None,
        }),
        Err(e) => Ok(SourceConcurrencyReport {
            value: json!({
                "ok": false,
                "status": "pending_recovery",
                "mode": "precise_patch",
                "source_write_performed": wrote,
                "write_outcome": "pending_recovery",
                "error": e.report(),
                "recovery_directory": format!(".awr/mutations/{owner}"),
            }),
            failure: Some(e),
        }),
    }
}

/// Bind every shard path to its Store-registered source before any journal or filesystem write.
///
/// Rejects forged path+fingerprint pairs against unregistered siblings, adapter mismatches,
/// and paths that escape the project root or the `.awr` writable boundary.
fn bind_registered_shard_paths(
    store: &Store,
    root: &Path,
    project_id: Id,
    adapter: &str,
    candidate: &ShardCandidate,
) -> Result<()> {
    let root = root.canonicalize()?;
    if candidate.adapter != adapter {
        return Err(Error::SourceConflict(
            "shard candidate adapter does not match activation adapter".into(),
        ));
    }
    require_write_mode(adapter, SourceWriteMode::ShardedFiles)?;
    for shard in &candidate.shards {
        let source = store.source(project_id, shard.source_id).map_err(|_| {
            Error::SourceConflict("shard source_id is not registered in the project Store".into())
        })?;
        if source.adapter != adapter {
            return Err(Error::SourceConflict(
                "shard source adapter does not match activation adapter".into(),
            ));
        }
        require_write_mode(&source.adapter, SourceWriteMode::ShardedFiles)?;
        let (locator, spec, _snapshot) = inspect_registered_source(&root, &source)?;
        if spec.adapter != adapter {
            return Err(Error::SourceConflict(
                "registered manifest adapter does not match shard activation adapter".into(),
            ));
        }
        let Locator::File(registered) = locator else {
            return Err(Error::MutationUnsupported(
                "sharded writes require local file sources".into(),
            ));
        };
        let relative = registered
            .strip_prefix(&root)
            .map_err(|_| Error::RuleViolation("shard source path escapes project root".into()))?;
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(Error::RuleViolation(
                "shard source path escapes project root".into(),
            ));
        }
        if awr_source::is_runtime_owned_path(&root, &registered) {
            return Err(Error::RuleViolation(
                "runtime-owned files cannot be shard write targets; relocate authored documents outside .awr (except registered .awr/intake sources) and reindex".into(),
            ));
        }
        if relative != shard.path.as_path() {
            return Err(Error::SourceConflict(
                "shard path does not match the registered source locator".into(),
            ));
        }
        let claimed = root.join(&shard.path);
        if !claimed.starts_with(&root) || awr_source::is_runtime_owned_path(&root, &claimed) {
            return Err(Error::RuleViolation(
                "shard path escapes the writable project boundary".into(),
            ));
        }
        let path_spec = document_path_registration(&root, &registered)?;
        if path_spec.adapter != source.adapter {
            return Err(Error::SourceConflict(
                "shard path is outside the registered writable adapter boundary".into(),
            ));
        }
    }
    Ok(())
}

/// Persist or reuse durable `N.after` artifacts (create_new only when missing).
fn ensure_shard_after_artifacts(dir: &Dir, candidate: &ShardCandidate) -> Result<()> {
    for (i, shard) in candidate.shards.iter().enumerate() {
        let name = format!("{i}.after");
        let after = shard.after_bytes()?;
        match dir.open(&name) {
            Ok(file) => {
                let mut bytes = Vec::new();
                file.take(16 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
                if bytes.len() > 16 * 1024 * 1024 {
                    return Err(Error::InvalidInput(
                        "durable shard after artifact exceeds 16 MiB".into(),
                    ));
                }
                if bytes != after {
                    return Err(Error::SourceConflict(
                        "durable shard after artifact differs from reviewed candidate".into(),
                    ));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut file = new_file(dir, OsStr::new(&name))?;
                file.write_all(&after)?;
                file.sync_all()?;
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Form and atomically activate a supported shard candidate under a recovery journal.
pub fn activate_shard_candidate(
    store: &mut Store,
    root: &Path,
    project_id: Id,
    request_key: &str,
    adapter: &str,
    shards: Vec<ShardWrite>,
    expected_revision: Revision,
) -> Result<SourceConcurrencyReport> {
    let root = root.canonicalize()?;
    let _mode = source_write_mode(adapter)?;
    require_write_mode(adapter, SourceWriteMode::ShardedFiles)?;
    let candidate = form_shard_candidate(adapter, shards)?;
    // Bind every shard to its registered Store source before any journal/fs mutation.
    bind_registered_shard_paths(store, &root, project_id, adapter, &candidate)?;

    let name = recovery_name(project_id, request_key)?;
    let owner = name.clone();
    let parent = recovery_root(&root)?;
    let dir = directory(&parent, &owner, &root.join(".awr/mutations").join(&owner))?;
    let _lock = named_lock(&root, &format!("{owner}.lock"))?;

    let receipt = if let Some(existing) = load_json::<ShardReceipt>(&dir, "receipt.json")? {
        if existing.phase == "completed"
            && existing.candidate.candidate_digest == candidate.candidate_digest
        {
            return Ok(SourceConcurrencyReport {
                value: json!({
                    "ok": true,
                    "status": "completed",
                    "already_recorded": true,
                    "mode": "sharded",
                    "candidate_digest": existing.candidate.candidate_digest,
                    "project_revision": existing.project_revision,
                    "applied": existing.applied,
                }),
                failure: None,
            });
        }
        if existing.phase != "completed"
            && existing.candidate.candidate_digest == candidate.candidate_digest
        {
            // Same reviewed candidate: resume durable receipt instead of wiping applied[].
            existing
        } else {
            // Pending receipt for this request_key is bound to a different candidate.
            // Refuse before any durable overwrite so recovery can still use the original.
            return Err(Error::SourceConflict(
                "request_key reused with a different shard candidate intent".into(),
            ));
        }
    } else {
        let receipt = ShardReceipt {
            version: 1,
            phase: "planned".into(),
            project_id,
            project_revision: expected_revision,
            request_key: request_key.into(),
            candidate: candidate.clone(),
            applied: vec![],
        };
        save_json(&dir, "receipt.json", &receipt)?;
        receipt
    };
    ensure_shard_after_artifacts(&dir, &candidate)?;

    activate_shard_candidate_locked(
        store,
        &root,
        &dir,
        &owner,
        receipt,
        expected_revision,
        false,
    )
}

/// Resume shard activation while already holding the recovery named lock.
///
/// Reuses durable `N.after` artifacts and the receipt's applied set; does not
/// re-enter [`activate_shard_candidate`]'s non-reentrant lock acquisition.
fn activate_shard_candidate_locked(
    store: &mut Store,
    root: &Path,
    dir: &Dir,
    owner: &str,
    mut receipt: ShardReceipt,
    expected_revision: Revision,
    recovered: bool,
) -> Result<SourceConcurrencyReport> {
    let candidate = receipt.candidate.clone();
    candidate.validate()?;
    bind_registered_shard_paths(
        store,
        root,
        receipt.project_id,
        &candidate.adapter,
        &candidate,
    )?;
    ensure_shard_after_artifacts(dir, &candidate)?;
    let prior_phase = receipt.phase.clone();

    let mut wrote = false;
    let result = (|| -> Result<Revision> {
        let actual = store.project(receipt.project_id)?.project_revision;
        if actual != expected_revision {
            return Err(Error::RevisionConflict {
                expected: expected_revision,
                actual,
            });
        }
        let states = observe_candidate(root, &candidate)?;
        for (i, state) in states.iter().enumerate() {
            match state {
                ShardObservation::Before | ShardObservation::After => {}
                ShardObservation::External => {
                    return Err(Error::SourceConflict(
                        "shard activation preserves external source changes; recover explicitly"
                            .into(),
                    ));
                }
                ShardObservation::Missing => {
                    return Err(Error::SourceUnavailable(format!(
                        "shard {} is missing before activation",
                        candidate.shards[i].path.display()
                    )));
                }
            }
        }
        for (i, shard) in candidate.shards.iter().enumerate() {
            let current = observe_shard(root, shard)?;
            if current == ShardObservation::External {
                return Err(Error::SourceConflict(
                    "source changed while shard candidate was in progress".into(),
                ));
            }
            if current == ShardObservation::Before {
                let path = root.join(&shard.path);
                let permissions = open_file_exact(&path)?.metadata()?.permissions();
                if permissions.readonly() {
                    return Err(Error::RuleViolation(
                        "shard destination is read-only".into(),
                    ));
                }
                let after = match dir.open(format!("{i}.after")) {
                    Ok(file) => {
                        let mut bytes = Vec::new();
                        file.take(16 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
                        if bytes.len() > 16 * 1024 * 1024 {
                            return Err(Error::InvalidInput(
                                "durable shard after artifact exceeds 16 MiB".into(),
                            ));
                        }
                        bytes
                    }
                    Err(_) => shard.after_bytes()?,
                };
                if fingerprint(&after) != shard.after_fingerprint {
                    return Err(Error::SourceConflict(
                        "durable shard after artifact fingerprint mismatch".into(),
                    ));
                }
                let mut replacement = SourceReplacement::prepare(&path, &after, permissions)?;
                if observe_shard(root, shard)? != ShardObservation::Before {
                    return Err(Error::SourceConflict(
                        "shard changed immediately before writing".into(),
                    ));
                }
                replacement.install()?;
                replacement.sync_parent()?;
                wrote = true;
            }
            if !receipt.applied.contains(&i) {
                receipt.applied.push(i);
            }
            save_json(dir, "receipt.json", &receipt)?;
        }
        receipt.phase = "applied".into();
        save_json(dir, "receipt.json", &receipt)?;
        let indexed = index_project(store, root, &Manifest::load(root)?, false)?;
        if !indexed.ok {
            return Err(Error::SourceStale(
                "shard files were written but indexing requires recovery".into(),
            ));
        }
        for state in observe_candidate(root, &candidate)? {
            if state != ShardObservation::After {
                return Err(Error::SourceConflict(
                    "activated shard candidate is not fully after-state".into(),
                ));
            }
        }
        receipt.phase = "completed".into();
        receipt.project_revision = indexed.project_revision;
        save_json(dir, "receipt.json", &receipt)?;
        Ok(indexed.project_revision)
    })();

    match result {
        Ok(revision) => {
            let mut value = json!({
                "ok": true,
                "status": "completed",
                "mode": "sharded",
                "already_recorded": false,
                "source_write_performed": wrote,
                "filesystem_atomic": false,
                "candidate_digest": candidate.candidate_digest,
                "applied": receipt.applied,
                "file_total": candidate.shards.len(),
                "project_revision": revision,
                "recovery_directory": format!(".awr/mutations/{owner}"),
            });
            if recovered {
                value["recovered"] = json!(true);
                value["prior_phase"] = json!(prior_phase);
            }
            Ok(SourceConcurrencyReport {
                value,
                failure: None,
            })
        }
        Err(e) => {
            let mut value = json!({
                "ok": false,
                "status": "pending_recovery",
                "mode": "sharded",
                "source_write_performed": wrote,
                "partial_apply": !receipt.applied.is_empty(),
                "applied": receipt.applied,
                "file_total": candidate.shards.len(),
                "write_outcome": "pending_recovery",
                "candidate_digest": candidate.candidate_digest,
                "error": e.report(),
                "recovery_directory": format!(".awr/mutations/{owner}"),
            });
            if recovered {
                value["recovered"] = json!(true);
                value["prior_phase"] = json!(prior_phase);
            }
            Ok(SourceConcurrencyReport {
                value,
                failure: Some(e),
            })
        }
    }
}

/// Resume a pending shard activation without overwriting external bytes.
pub fn recover_shard_candidate(
    store: &mut Store,
    root: &Path,
    project_id: Id,
    request_key: &str,
    expected_revision: Revision,
) -> Result<SourceConcurrencyReport> {
    let root = root.canonicalize()?;
    let name = recovery_name(project_id, request_key)?;
    let owner = name.clone();
    let parent = recovery_root(&root)?;
    let dir = parent
        .open_dir_nofollow(&owner)
        .map_err(|e| Error::SourceUnavailable(format!("missing shard recovery journal: {e}")))?;
    let _lock = named_lock(&root, &format!("{owner}.lock"))?;
    let receipt: ShardReceipt = load_json(&dir, "receipt.json")?
        .ok_or_else(|| Error::NotFound("shard concurrency receipt".into()))?;
    if receipt.project_id != project_id {
        return Err(Error::SourceConflict(
            "shard recovery receipt belongs to another project".into(),
        ));
    }
    if receipt.phase == "completed" {
        return Ok(SourceConcurrencyReport {
            value: json!({
                "ok": true,
                "status": "completed",
                "already_recorded": true,
                "mode": "sharded",
                "candidate_digest": receipt.candidate.candidate_digest,
                "project_revision": receipt.project_revision,
            }),
            failure: None,
        });
    }
    // Resume under the lock we already hold; do not re-enter activate_shard_candidate.
    activate_shard_candidate_locked(store, &root, &dir, &owner, receipt, expected_revision, true)
}

/// Classify whether a live observation blocks a stale whole-file install.
pub fn classify_whole_file_gate(expected_before: &str, observed: &str) -> Result<&'static str> {
    match refuse_stale_whole_file(expected_before, observed) {
        Ok(()) => Ok("installable"),
        Err(Error::SourceConflict(_)) => Ok("refuse_stale"),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use awr_source::fingerprint;

    #[test]
    fn whole_file_gate_refuses_stale_fingerprints() {
        let a = fingerprint(b"one");
        let b = fingerprint(b"two");
        assert_eq!(classify_whole_file_gate(&a, &a).unwrap(), "installable");
        assert_eq!(classify_whole_file_gate(&a, &b).unwrap(), "refuse_stale");
    }

    #[test]
    fn unsupported_adapter_refused_before_activation() {
        let err = require_write_mode("yaml-workstream-ledger-v1", SourceWriteMode::ShardedFiles)
            .unwrap_err();
        assert!(matches!(err, Error::MutationUnsupported(_)));
    }
}
