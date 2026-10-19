use crate::{REGISTERED_CONTENT_READ_CAP, Runtime};
use awr_core::*;
use sha2::{Digest, Sha256};
use std::{io::Read, path::Path};

impl Runtime<'_> {
    /// Load current trusted policy before calling. The selected scope authorizes
    /// metadata before the filesystem is touched; existing path/hash checks still
    /// apply. This does not grant direct OS-file access or authorize execution.
    pub fn read_artifact_in_workstream(
        &self,
        access: &WorkstreamAccess,
        selection: &awr_store::WorkstreamReadSelection,
        id: Id,
        max_bytes: u64,
    ) -> Result<(Artifact, Vec<u8>)> {
        let view =
            self.store
                .read_workstream(self.project, access, selection, 256 * 1024 * 1024)?;
        let artifact = view.artifact(id)?;
        ensure_public_data(&artifact)?;
        let bytes = read_registered_file(
            self.store,
            self.project,
            &artifact.locator,
            Some(artifact.size),
            Some(&artifact.sha256),
            max_bytes,
        )?;
        let current =
            self.store
                .read_workstream(self.project, access, selection, 256 * 1024 * 1024)?;
        let checked = current.artifact(id)?;
        if checked.sha256 != artifact.sha256
            || checked.locator != artifact.locator
            || checked.revision != artifact.revision
        {
            return Err(Error::SourceConflict(
                "artifact changed during scoped read".into(),
            ));
        }
        Ok((artifact, bytes))
    }

    pub fn read_evidence_report_in_workstream(
        &self,
        access: &WorkstreamAccess,
        selection: &awr_store::WorkstreamReadSelection,
        key: &str,
        max_bytes: u64,
    ) -> Result<(EvidenceRecord, Vec<u8>)> {
        let view =
            self.store
                .read_workstream(self.project, access, selection, 256 * 1024 * 1024)?;
        let record = view.evidence(key)?;
        ensure_public_data(&record)?;
        let bytes = read_registered_file(
            self.store,
            self.project,
            &record.item.locator,
            None,
            record.item.sha256.as_deref(),
            max_bytes,
        )?;
        let current =
            self.store
                .read_workstream(self.project, access, selection, 256 * 1024 * 1024)?;
        let checked = current.evidence(&record.item.id.to_string())?;
        if checked.item.revision != record.item.revision
            || checked.item.locator != record.item.locator
            || checked.item.sha256 != record.item.sha256
        {
            return Err(Error::SourceConflict(
                "evidence changed during scoped read".into(),
            ));
        }
        Ok((record, bytes))
    }

    /// Explicit artifact body read. No content is returned until size and digest match its record.
    pub fn read_artifact(&self, id: Id, max_bytes: u64) -> Result<(Artifact, Vec<u8>)> {
        let artifact = self.store.artifact(self.project, id)?;
        ensure_public_data(&artifact)?;
        let bytes = read_registered_file(
            self.store,
            self.project,
            &artifact.locator,
            Some(artifact.size),
            Some(&artifact.sha256),
            max_bytes,
        )?;
        Ok((artifact, bytes))
    }

    /// Read a registered local report on demand. Absence of a recorded digest is not verification.
    pub fn read_evidence_report(
        &self,
        key: &str,
        max_bytes: u64,
    ) -> Result<(EvidenceRecord, Vec<u8>)> {
        let record = self.store.evidence(self.project, key)?;
        ensure_public_data(&record)?;
        let bytes = read_registered_file(
            self.store,
            self.project,
            &record.item.locator,
            None,
            record.item.sha256.as_deref(),
            max_bytes,
        )?;
        Ok((record, bytes))
    }
}

pub(crate) fn read_registered_file(
    store: &awr_store::Store,
    project: Id,
    locator: &str,
    expected_size: Option<u64>,
    expected_sha: Option<&str>,
    max_bytes: u64,
) -> Result<Vec<u8>> {
    if max_bytes == 0 || max_bytes > REGISTERED_CONTENT_READ_CAP {
        return Err(Error::InvalidInput(
            "read limit must be 1..16777216 bytes".into(),
        ));
    }
    if expected_size.is_some_and(|size| size > max_bytes) {
        return Err(Error::InvalidInput(format!(
            "artifact exceeds {max_bytes} byte read limit"
        )));
    }
    let project = store.project(project)?;
    let root = project.root.canonicalize()?;
    if root != project.root {
        return Err(Error::SourceConflict(
            "project root moved since registration".into(),
        ));
    }
    if locator.starts_with("project://") {
        // One read path for project locators: `Locator` owns the syntax, the
        // project-root rule and the byte read, so this function carries no
        // second parser or containment rule of its own. Bytes that differ from
        // the declared `sha256` fail below and never fall back to another copy.
        let manifest = awr_source::Manifest::load(&root)?;
        let snapshot =
            awr_source::Locator::parse(&root, &manifest, locator)?.read(&root, max_bytes)?;
        let bytes = snapshot.bytes;
        if expected_size.is_some_and(|size| size != bytes.len() as u64) {
            return Err(Error::SourceConflict(
                "artifact size differs from its registered metadata".into(),
            ));
        }
        verify_registered_sha(&bytes, expected_sha)?;
        ensure_public_bytes(&bytes)?;
        return Ok(bytes);
    }
    if locator.contains("://") {
        return Err(Error::Unsupported("body reads currently support registered local paths and project locators; use the report locator for remote content".into()));
    }
    let path = if Path::new(locator).is_absolute() {
        Path::new(locator).to_path_buf()
    } else {
        root.join(locator)
    }
    .canonicalize()?;
    if !within_project_or_authorized(&root, &path)? {
        return Err(Error::RuleViolation(
            "registered file is outside project and authorized roots".into(),
        ));
    }
    if !path.metadata()?.is_file() {
        return Err(Error::InvalidInput(
            "registered content is not a regular file".into(),
        ));
    }
    let file = awr_source::open_file_exact(&path)?;
    let before = file.metadata()?;
    if !before.is_file() {
        return Err(Error::InvalidInput(
            "registered content is not a regular file".into(),
        ));
    }
    if before.len() > max_bytes {
        return Err(Error::InvalidInput(format!(
            "content has {} bytes; limit is {max_bytes}",
            before.len()
        )));
    }
    if expected_size.is_some_and(|size| size != before.len()) {
        return Err(Error::SourceConflict(
            "artifact size differs from its registered metadata".into(),
        ));
    }
    let mut bytes = Vec::with_capacity(before.len() as usize);
    (&file).take(max_bytes + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(Error::InvalidInput("content grew beyond read limit".into()));
    }
    let after = file.metadata()?;
    if bytes.len() as u64 != before.len()
        || after.len() != before.len()
        || after.modified()? != before.modified()?
    {
        return Err(Error::SourceConflict(
            "registered content changed during reading".into(),
        ));
    }
    verify_registered_sha(&bytes, expected_sha)?;
    ensure_public_bytes(&bytes)?;
    Ok(bytes)
}

/// The one containment rule for registered content: a canonical path is allowed
/// when it stays under the project root or under one of the manifest's
/// authorized roots.
///
/// Both the read path and the registration guard call this, so registration
/// cannot store a locator that a later read is guaranteed to refuse. Project
/// locators do not go through this rule: they resolve against the project root
/// alone, which keeps one stored string on the same bytes on every node.
pub(crate) fn within_project_or_authorized(root: &Path, path: &Path) -> Result<bool> {
    if path.starts_with(root) {
        return Ok(true);
    }
    if !root.join(".awr/project.toml").exists() {
        return Ok(false);
    }
    Ok(awr_source::Manifest::load(root)?
        .project
        .authorized_roots
        .iter()
        .any(|extra| {
            root.join(extra)
                .canonicalize()
                .is_ok_and(|resolved| path.starts_with(resolved))
        }))
}

/// Refuse bytes that no longer match the SHA256 bound at registration.
///
/// Both the local-path read and the project-locator read call this, so a node
/// whose checkout holds different bytes for the same relative path fails here
/// instead of falling back to another copy.
fn verify_registered_sha(bytes: &[u8], expected_sha: Option<&str>) -> Result<()> {
    let Some(expected) = expected_sha else {
        return Ok(());
    };
    let actual = format!("{:x}", Sha256::digest(bytes));
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(Error::SourceConflict(format!(
            "registered content bytes no longer match the SHA256 bound at registration: expected {expected}, actual {actual}; the file must stay byte-identical, so check for added or removed trailing newlines, whitespace or line-ending changes since the evidence was registered"
        )));
    }
    Ok(())
}
