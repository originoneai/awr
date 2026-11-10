use crate::{ARTIFACT_IMPORT_CAP, Runtime, fs_sync::sync_directory};
use awr_core::*;
use awr_source::{open_dir_exact, open_file_exact};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub struct ArtifactFile {
    pub path: PathBuf,
    pub artifact_type: String,
    pub mime: String,
    pub source_event_id: Id,
    pub max_bytes: u64,
}
struct PendingFile {
    directory: Dir,
    name: String,
    output: Option<File>,
    registered: bool,
}
impl PendingFile {
    fn new(directory: Dir, id: Id) -> Result<Self> {
        let name = id.to_string();
        let mut options = OpenOptions::new();
        options
            .create_new(true)
            .write(true)
            .follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let output = directory.open_with(&name, &options)?.into_std();
        Ok(Self {
            directory,
            name,
            output: Some(output),
            registered: false,
        })
    }
}
impl Drop for PendingFile {
    fn drop(&mut self) {
        drop(self.output.take());
        if !self.registered {
            let _ = self.directory.remove_file(&self.name);
        }
    }
}

pub(crate) fn storage_directory(root: &Path) -> Result<Dir> {
    let mut parent = open_dir_exact(root)?;
    for name in [".awr", "artifacts"] {
        match parent.create_dir(name) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error.into()),
        }
        let meta = parent.symlink_metadata(name)?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(Error::RuleViolation(
                "artifact storage must use real directories".into(),
            ));
        }
        parent = parent.open_dir_nofollow(name)?;
    }
    Ok(parent)
}

impl Runtime<'_> {
    /// Copy a bounded file into immutable managed storage, then register only its metadata.
    pub fn import_artifact(
        &mut self,
        expected: Revision,
        request: ArtifactFile,
    ) -> Result<(Artifact, Event)> {
        ensure_public_data(&(&request.path, &request.artifact_type, &request.mime))?;
        let project = self.store.project(self.project)?;
        if project.project_revision != expected {
            return Err(Error::RevisionConflict {
                expected,
                actual: project.project_revision,
            });
        }
        self.store.event(self.project, request.source_event_id)?;
        if request.max_bytes == 0 || request.max_bytes > ARTIFACT_IMPORT_CAP {
            return Err(Error::InvalidInput(format!(
                "artifact import limit must be 1..{ARTIFACT_IMPORT_CAP} bytes"
            )));
        }
        let root = project.root.canonicalize()?;
        if root != project.root {
            return Err(Error::SourceConflict(
                "project root moved since registration".into(),
            ));
        }
        let path = if request.path.is_absolute() {
            request.path.clone()
        } else {
            root.join(&request.path)
        }
        .canonicalize()?;
        let allowed = path.starts_with(&root)
            || (root.join(".awr/project.toml").exists()
                && awr_source::Manifest::load(&root)?
                    .project
                    .authorized_roots
                    .iter()
                    .any(|p| {
                        root.join(p)
                            .canonicalize()
                            .is_ok_and(|r| path.starts_with(r))
                    }));
        if !allowed {
            return Err(Error::RuleViolation(
                "artifact source is outside project and authorized roots".into(),
            ));
        }
        let mut source = open_file_exact(&path)?;
        let before = source.metadata()?;
        if !before.is_file() || before.len() > request.max_bytes {
            return Err(Error::InvalidInput(
                "artifact source must be a regular file within the byte limit".into(),
            ));
        }
        // Inspect the complete bounded snapshot before creating any managed file. A sliding
        // chunk window could miss split labels or a value after a long whitespace run.
        let mut bytes = Vec::with_capacity(before.len() as usize);
        (&mut source)
            .take(request.max_bytes + 1)
            .read_to_end(&mut bytes)?;
        let size = bytes.len() as u64;
        if size > request.max_bytes {
            return Err(Error::InvalidInput(
                "artifact grew beyond the byte limit".into(),
            ));
        }
        let after = source.metadata()?;
        if size != before.len()
            || after.len() != before.len()
            || after.modified()? != before.modified()?
        {
            return Err(Error::SourceConflict(
                "artifact source changed during copying".into(),
            ));
        }
        ensure_public_bytes(&bytes)?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let directory = storage_directory(&root)?;
        let storage_id = Id::new();
        let destination = root.join(".awr/artifacts").join(storage_id.to_string());
        let mut pending = PendingFile::new(directory, storage_id)?;
        pending.output.as_mut().unwrap().write_all(&bytes)?;
        pending.output.as_ref().unwrap().sync_all()?;
        sync_directory(&pending.directory)?;
        drop(pending.output.take());
        let locator = destination
            .strip_prefix(&root)
            .map_err(|_| Error::RuleViolation("artifact locator escapes project".into()))?
            .to_str()
            .ok_or_else(|| Error::InvalidInput("artifact path must be UTF-8".into()))?
            .to_string();
        let result = self.store.record_artifact(
            self.project,
            expected,
            ArtifactDraft {
                artifact_type: request.artifact_type,
                locator,
                sha256: digest,
                size,
                mime: request.mime,
                source_event_id: request.source_event_id,
            },
        )?;
        pending.registered = true;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Temporary(PathBuf);
    impl Temporary {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("awr-artifact-handles-{}", Id::new()));
            fs::create_dir(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }
    }
    impl Drop for Temporary {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn failed_creation_never_owns_or_removes_an_existing_artifact() {
        let root = Temporary::new();
        let directory = storage_directory(&root.0).unwrap();
        let id = Id::new();
        let path = root.0.join(".awr/artifacts").join(id.to_string());
        fs::write(&path, b"Existing artifact").unwrap();
        assert!(PendingFile::new(directory, id).is_err());
        assert_eq!(fs::read(path).unwrap(), b"Existing artifact");
    }

    #[cfg(unix)]
    #[test]
    fn failed_import_cleanup_uses_the_held_directory_after_redirection() {
        use std::os::unix::fs::symlink;
        let root = Temporary::new();
        let directory = storage_directory(&root.0).unwrap();
        let id = Id::new();
        let mut pending = PendingFile::new(directory, id).unwrap();
        pending
            .output
            .as_mut()
            .unwrap()
            .write_all(b"Pending artifact")
            .unwrap();
        let retained = root.0.join(".awr/retained");
        fs::rename(root.0.join(".awr/artifacts"), &retained).unwrap();
        let replacement = root.0.join("replacement");
        fs::create_dir(&replacement).unwrap();
        fs::write(replacement.join(id.to_string()), b"Unrelated file").unwrap();
        symlink(&replacement, root.0.join(".awr/artifacts")).unwrap();
        drop(pending);
        assert!(!retained.join(id.to_string()).exists());
        assert_eq!(
            fs::read(replacement.join(id.to_string())).unwrap(),
            b"Unrelated file"
        );
    }
}
