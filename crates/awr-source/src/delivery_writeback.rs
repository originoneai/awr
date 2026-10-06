//! Bounded delivery references and cooperative, exact-source writes.
//!
//! This layer neither verifies domain receipts nor changes runtime state. The
//! authenticated publisher must resolve every reference before preparing a note.
use crate::{LedgerWritebackPatch, YAML_READ_CAP, fingerprint, open_dir_exact};
use awr_core::{Error, Id, Result};
use cap_fs_ext::{FollowSymlinks, MetadataExt, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

/// A reference to a domain receipt, not a receipt or caller verification flag.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryCompletionReference {
    pub receipt_id: String,
    pub evidence_id: String,
    pub result_digest: String,
    pub artifact_id: String,
    pub artifact_sha256: String,
}

/// One bounded current note. History is retained by the publisher journal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliverySourceNote {
    pub version: u32,
    pub publication_id: String,
    pub work_external_key: String,
    pub contract_snapshot_id: String,
    pub candidate_id: String,
    pub candidate_version: String,
    pub candidate_digest: String,
    pub selection_version: String,
    pub metadata_revision: String,
    pub observation_receipt_ids: Vec<String>,
    pub fact_ids: Vec<String>,
    pub completion_reference: Option<DeliveryCompletionReference>,
}

fn invalid() -> Error {
    Error::InvalidInput("invalid delivery source note or bounds".into())
}

fn identity(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        return Err(invalid());
    }
    Ok(())
}

fn digest(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid());
    }
    Ok(())
}

fn revision(value: &str) -> Result<u64> {
    if value.is_empty()
        || value.len() > 19
        || value.starts_with('0')
        || !value.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(invalid());
    }
    value
        .parse::<i64>()
        .map(|v| v as u64)
        .map_err(|_| invalid())
}

impl DeliverySourceNote {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1 {
            return Err(invalid());
        }
        for value in [
            &self.publication_id,
            &self.work_external_key,
            &self.contract_snapshot_id,
            &self.candidate_id,
            &self.candidate_version,
        ] {
            identity(value)?;
        }
        digest(&self.candidate_digest)?;
        revision(&self.selection_version)?;
        revision(&self.metadata_revision)?;
        for refs in [&self.observation_receipt_ids, &self.fact_ids] {
            if refs.len() > 32
                || refs.iter().collect::<std::collections::BTreeSet<_>>().len() != refs.len()
            {
                return Err(invalid());
            }
            for value in refs {
                identity(value)?;
            }
        }
        if self.observation_receipt_ids.is_empty() != self.fact_ids.is_empty()
            || self.fact_ids.is_empty() && self.completion_reference.is_none()
        {
            return Err(invalid());
        }
        if let Some(reference) = &self.completion_reference {
            for value in [
                &reference.receipt_id,
                &reference.evidence_id,
                &reference.artifact_id,
            ] {
                identity(value)?;
            }
            digest(&reference.result_digest)?;
            digest(&reference.artifact_sha256)?;
        }
        if serde_json::to_vec(self)?.len() > 16384 {
            return Err(invalid());
        }
        awr_core::ensure_public_data(self)
    }
}

/// Patch only the exact work's typed reference note. Status, contracts, ownership
/// and all unrelated bytes remain untouched. A reference never finalizes work.
pub fn prepare_delivery_source_note(
    bytes: &[u8],
    note: &DeliverySourceNote,
) -> Result<LedgerWritebackPatch> {
    note.validate()?;
    if bytes.len() as u64 > YAML_READ_CAP {
        return Err(invalid());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    let document = serde_yaml_ng::from_slice::<serde_yaml_ng::Value>(bytes)
        .and_then(Value::deserialize)
        .map_err(|_| invalid())?;
    let items = document
        .get("work_items")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?;
    let matches: Vec<_> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.get("id").and_then(Value::as_str) == Some(&note.work_external_key))
        .collect();
    if matches.len() != 1 {
        return Err(Error::SourceConflict(
            "delivery note requires one exact source work record".into(),
        ));
    }
    let (index, item) = matches[0];
    let value = json!(note);
    let mut after_bytes = bytes.to_vec();
    if let Some(old) = item.get("delivery_sync") {
        let old_note: DeliverySourceNote = serde_json::from_value(old.clone()).map_err(|_| {
            Error::SourceConflict(
                "existing delivery_sync field is not a supported typed note".into(),
            )
        })?;
        old_note.validate()?;
        if old_note.work_external_key != note.work_external_key {
            return Err(invalid());
        }
        if old != &value
            && old_note.contract_snapshot_id == note.contract_snapshot_id
            && (revision(&note.metadata_revision)? <= revision(&old_note.metadata_revision)?
                || revision(&note.selection_version)? < revision(&old_note.selection_version)?
                || note.selection_version == old_note.selection_version
                    && note.candidate_digest != old_note.candidate_digest)
        {
            return Err(Error::SourceConflict(
                "delivery note revision or selection is stale".into(),
            ));
        }
        if old == &value {
            return Ok(LedgerWritebackPatch {
                before_fingerprint: fingerprint(bytes),
                after_fingerprint: fingerprint(bytes),
                before_bytes: bytes.to_vec(),
                after_bytes,
                changed_external_keys: vec![],
                refused_runtime_fields: vec![],
            });
        }
    }
    let mut fields = Map::new();
    fields.insert("delivery_sync".into(), value);
    after_bytes =
        crate::yaml_edit::edit_fields(text, &format!("/work_items/{index}"), &fields)?.into_bytes();
    if after_bytes.len() as u64 > YAML_READ_CAP {
        return Err(invalid());
    }
    Ok(LedgerWritebackPatch {
        before_fingerprint: fingerprint(bytes),
        after_fingerprint: fingerprint(&after_bytes),
        before_bytes: bytes.to_vec(),
        after_bytes,
        changed_external_keys: vec![note.work_external_key.clone()],
        refused_runtime_fields: vec![],
    })
}

/// Persist with an intent to detect directory/lock replacement during recovery.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceFileIdentity {
    pub root_device: u64,
    pub root_inode: u64,
    pub parent_device: u64,
    pub parent_inode: u64,
    pub lock_device: u64,
    pub lock_inode: u64,
}

/// A stable advisory lock shared by cooperating planning and delivery writers.
/// Noncooperating writers may still race a rename; fingerprints detect drift
/// observed before/after the effect, rather than promising physical isolation.
pub struct LockedSourceFile {
    root_path: PathBuf,
    parent_path: PathBuf,
    parent: Dir,
    leaf: String,
    lock_name: String,
    _lock: File,
    identity: SourceFileIdentity,
}

fn conflict() -> Error {
    Error::SourceConflict("bound source directory or cooperative writer lock changed".into())
}

impl LockedSourceFile {
    pub fn open(root: &Path, relative: &str) -> Result<Self> {
        if relative.is_empty()
            || relative.contains(['\\', '\0'])
            || relative
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
            || Path::new(relative)
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(invalid());
        }
        let root_dir = open_dir_exact(root)?;
        let path = root.join(relative);
        let parent_path = path.parent().ok_or_else(invalid)?.to_path_buf();
        let leaf = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(invalid)?
            .to_owned();
        let parent = open_dir_exact(&parent_path)?;
        // The leaf name under its exact parent, not a root-relative spelling:
        // alternate authorized roots for the same file must share this lock.
        let lock_name = format!(".awr-source-{}.lock", &fingerprint(leaf.as_bytes())[7..]);
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create(true)
            .follow(FollowSymlinks::No);
        let cap_lock = parent.open_with(&lock_name, &options)?;
        let metadata = cap_lock.metadata()?;
        let lock = cap_lock.into_std();
        if !metadata.is_file() || metadata.nlink() != 1 {
            return Err(conflict());
        }
        match lock.try_lock() {
            Ok(()) => (),
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(Error::SourceConflict("source writer is busy".into()));
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
        let root_metadata = root_dir.dir_metadata()?;
        let parent_metadata = parent.dir_metadata()?;
        let guard = Self {
            root_path: root.into(),
            parent_path,
            parent,
            leaf,
            lock_name,
            _lock: lock,
            identity: SourceFileIdentity {
                root_device: root_metadata.dev(),
                root_inode: root_metadata.ino(),
                parent_device: parent_metadata.dev(),
                parent_inode: parent_metadata.ino(),
                lock_device: metadata.dev(),
                lock_inode: metadata.ino(),
            },
        };
        guard.check_identity()?;
        Ok(guard)
    }

    pub fn identity(&self) -> &SourceFileIdentity {
        &self.identity
    }

    pub fn verify_identity(&self, expected: &SourceFileIdentity) -> Result<()> {
        if &self.identity != expected {
            return Err(conflict());
        }
        self.check_identity()
    }

    fn check_identity(&self) -> Result<()> {
        let root = open_dir_exact(&self.root_path)?.dir_metadata()?;
        let parent = open_dir_exact(&self.parent_path)?.dir_metadata()?;
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        let lock = self
            .parent
            .open_with(&self.lock_name, &options)?
            .metadata()?;
        if root.dev() != self.identity.root_device
            || root.ino() != self.identity.root_inode
            || parent.dev() != self.identity.parent_device
            || parent.ino() != self.identity.parent_inode
            || !lock.is_file()
            || lock.nlink() != 1
            || lock.dev() != self.identity.lock_device
            || lock.ino() != self.identity.lock_inode
        {
            return Err(conflict());
        }
        Ok(())
    }

    pub fn read(&self) -> Result<Vec<u8>> {
        self.check_identity()?;
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        let file = self.parent.open_with(&self.leaf, &options)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.nlink() != 1 {
            return Err(conflict());
        }
        let mut bytes = Vec::new();
        file.take(YAML_READ_CAP + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > YAML_READ_CAP {
            return Err(invalid());
        }
        self.check_identity()?;
        Ok(bytes)
    }

    /// Exact-byte CAS, file sync and confined atomic replacement. Unix also
    /// syncs the parent; Windows retains its file-sync/rename durability limit.
    pub fn replace(&self, expected_fingerprint: &str, after: &[u8]) -> Result<()> {
        if after.len() as u64 > YAML_READ_CAP {
            return Err(invalid());
        }
        if fingerprint(&self.read()?) != expected_fingerprint {
            return Err(Error::SourceConflict(
                "source fingerprint changed before replacement".into(),
            ));
        }
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        let permissions = self
            .parent
            .open_with(&self.leaf, &options)?
            .metadata()?
            .permissions();
        if permissions.readonly() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "source file is read-only",
            )
            .into());
        }
        let temporary = format!("{}.tmp-{}", self.lock_name, Id::new());
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        let mut file = self.parent.open_with(&temporary, &options)?;
        let result = (|| -> Result<()> {
            file.write_all(after)?;
            file.set_permissions(permissions)?;
            file.sync_all()?;
            drop(file);
            if fingerprint(&self.read()?) != expected_fingerprint {
                return Err(Error::SourceConflict(
                    "source fingerprint changed before replacement".into(),
                ));
            }
            self.parent.rename(&temporary, &self.parent, &self.leaf)?;
            #[cfg(unix)]
            self.parent.try_clone()?.into_std_file().sync_all()?;
            self.check_identity()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = self.parent.remove_file(&temporary);
        }
        result
    }
}
