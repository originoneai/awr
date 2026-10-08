//! An optimistic, bounded capture; SQLite never opens the originating files.
use awr_core::{Error, Id, Result};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    thread,
    time::{Duration, SystemTime},
};

#[derive(PartialEq, Eq)]
struct Stamp {
    length: u64,
    modified: SystemTime,
    identity: (u64, u64),
}
fn stamp(path: &Path) -> Result<Option<Stamp>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(value) => value,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if !metadata.is_file() {
        return Err(Error::RuleViolation(
            "snapshot requires regular database files".into(),
        ));
    }
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        (metadata.dev(), metadata.ino())
    };
    #[cfg(not(unix))]
    let identity = (0, 0);
    Ok(Some(Stamp {
        length: metadata.len(),
        modified: metadata.modified()?,
        identity,
    }))
}
pub(super) struct Files {
    pub database: PathBuf,
    directory: PathBuf,
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}
impl Files {
    pub fn capture(database: &Path, max_bytes: u64) -> Result<Self> {
        Self::capture_after_stamp(database, max_bytes, || ())
    }
    /// `between` runs once the files have been stamped and before any is read: the moment a writer can take them away.
    fn capture_after_stamp(
        database: &Path,
        max_bytes: u64,
        between: impl FnOnce(),
    ) -> Result<Self> {
        let sidecar = |suffix: &str| {
            let mut name = database.as_os_str().to_owned();
            name.push(suffix);
            PathBuf::from(name)
        };
        // A journal requires recovery and is not silently interpreted as a settled WAL view.
        if stamp(&sidecar("-journal"))?.is_some() {
            return Err(Error::SourceConflict(
                "database has a rollback journal; retry after its writer finishes".into(),
            ));
        }
        let paths = [database.to_path_buf(), sidecar("-wal")];
        let before = paths.iter().map(|p| stamp(p)).collect::<Result<Vec<_>>>()?;
        if before[0].is_none() {
            return Err(Error::NotFound("AWR database".into()));
        }
        let size: u64 = before.iter().flatten().map(|s| s.length).sum();
        if max_bytes == 0 || size > max_bytes {
            return Err(Error::InvalidInput(format!(
                "database and WAL exceed the {max_bytes} byte snapshot limit"
            )));
        }
        between();
        let mut images = Vec::new();
        for (path, seen) in paths.iter().zip(&before) {
            images.push(
                seen.as_ref()
                    .map(|seen| read_image(path, seen))
                    .transpose()?,
            );
        }
        // All input files must remain unchanged across both complete reads. An active
        // writer is a retryable conflict, not a corrupt or permanently failed source.
        for ((path, image), seen) in paths.iter().zip(&images).zip(&before) {
            if let (Some(image), Some(seen)) = (image, seen) {
                if &read_image(path, seen)? != image {
                    return Err(changed());
                }
            }
        }
        let after = paths.iter().map(|p| stamp(p)).collect::<Result<Vec<_>>>()?;
        if before != after || stamp(&sidecar("-journal"))?.is_some() {
            return Err(changed());
        }
        let directory = std::env::temp_dir().join(format!("awr-preview-{}", Id::new()));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&directory)?;
        let result = Self {
            database: directory.join("state.db"),
            directory,
        };
        for (name, image) in ["state.db", "state.db-wal"].iter().zip(images) {
            if let Some(image) = image {
                let mut options = fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                options
                    .open(result.directory.join(name))?
                    .write_all(&image)?;
            }
        }
        Ok(result)
    }
}
fn changed() -> Error {
    Error::SourceConflict("database changed during read-only capture; retry the preview".into())
}
const READ_ATTEMPTS: u32 = 3;
const READ_PAUSE: Duration = Duration::from_millis(10);
/// One complete image of a file that `stamp` saw. A file that is gone, different or shorter afterwards is the retryable race
/// with a writer (the last connection to close deletes the WAL), not an I/O failure. A file that is still there unchanged but
/// cannot be read is retried briefly (Windows refuses to open a file that is being deleted) before its error is reported.
fn read_image(path: &Path, seen: &Stamp) -> Result<Vec<u8>> {
    let mut failure = None;
    for attempt in 0..READ_ATTEMPTS {
        if attempt > 0 {
            thread::sleep(READ_PAUSE);
        }
        let mut bytes = Vec::new();
        match fs::File::open(path)
            .and_then(|file| file.take(seen.length + 1).read_to_end(&mut bytes))
        {
            Ok(_) if bytes.len() as u64 == seen.length => return Ok(bytes),
            Ok(_) => return Err(changed()),
            Err(error) => match stamp(path) {
                Ok(Some(now)) if now == *seen => failure = Some(error),
                _ => return Err(changed()),
            },
        }
    }
    Err(failure.expect("a read attempt failed").into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;
    #[test]
    fn live_wal_is_included_without_touching_original_files() {
        let directory = std::env::temp_dir().join(format!("awr-preview-test-{}", Id::new()));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("state.db");
        let store = Store::open(&path).unwrap();
        store.conn.execute_batch("PRAGMA wal_autocheckpoint=0; CREATE TABLE fixture (value TEXT); INSERT INTO fixture VALUES('committed in WAL');").unwrap();
        let files = || {
            fs::read_dir(&directory)
                .unwrap()
                .map(|p| {
                    let p = p.unwrap().path();
                    (p.clone(), fs::read(p).unwrap())
                })
                .collect::<std::collections::BTreeMap<_, _>>()
        };
        assert!(store.require_memory().is_err());
        let before = files();
        let copy = Store::preview_snapshot(&path, 8 * 1024 * 1024).unwrap();
        assert_eq!(
            copy.conn
                .query_row("SELECT value FROM fixture", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "committed in WAL"
        );
        assert!(copy.require_memory().is_ok());
        assert!(files() == before, "original database and sidecars changed");
        assert!(Store::preview_snapshot(&path, 1).is_err());
        drop(copy);
        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }

    fn scratch_directory() -> PathBuf {
        let directory = std::env::temp_dir().join(format!("awr-preview-test-{}", Id::new()));
        fs::create_dir(&directory).unwrap();
        directory
    }

    #[test]
    fn a_writer_closing_between_stamp_and_read_is_a_retryable_conflict() {
        let directory = scratch_directory();
        let (path, wal) = (directory.join("state.db"), directory.join("state.db-wal"));
        let store = Store::open(&path).unwrap();
        store
            .conn
            .execute_batch(
                "CREATE TABLE fixture (value TEXT); INSERT INTO fixture VALUES('first');",
            )
            .unwrap();
        store
            .conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .unwrap();
        // An update of an existing page lives in the WAL only, and closing the writer later rewrites that page in place:
        // the database file keeps its length, so the capture reaches the WAL before anything looks different.
        store
            .conn
            .execute_batch(
                "PRAGMA wal_autocheckpoint=0; UPDATE fixture SET value='committed in WAL';",
            )
            .unwrap();
        assert!(
            fs::metadata(&wal).unwrap().len() > 0,
            "the writer has live WAL frames"
        );
        let length = fs::metadata(&path).unwrap().len();
        // The last connection to close checkpoints and deletes the WAL: here exactly between the stamps and the reads.
        let mut writer = Some(store);
        match Files::capture_after_stamp(&path, 8 * 1024 * 1024, || drop(writer.take())) {
            Err(Error::SourceConflict(_)) => {}
            Err(other) => panic!("a vanished WAL must be the retryable conflict, got {other}"),
            Ok(_) => panic!("a capture whose WAL vanished must not succeed"),
        }
        assert!(
            !wal.exists(),
            "the closing writer removed the WAL, so the race was exercised"
        );
        assert_eq!(
            fs::metadata(&path).unwrap().len(),
            length,
            "only the WAL read could notice the writer"
        );
        // Retried once the writer is gone, the capture settles on the checkpointed database.
        let settled = Store::preview_snapshot(&path, 8 * 1024 * 1024).unwrap();
        assert_eq!(
            settled
                .conn
                .query_row("SELECT value FROM fixture", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "committed in WAL"
        );
        drop(settled);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_file_that_changed_since_it_was_stamped_is_a_retryable_conflict() {
        let directory = scratch_directory();
        let path = directory.join("state.db-wal");
        fs::write(&path, b"frames").unwrap();
        let seen = stamp(&path).unwrap().unwrap();
        assert_eq!(read_image(&path, &seen).unwrap(), b"frames");
        fs::write(&path, b"more frames than were stamped").unwrap();
        assert!(matches!(
            read_image(&path, &seen),
            Err(Error::SourceConflict(_))
        ));
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_file_that_stays_unchanged_is_the_io_error_it_is() {
        use std::os::unix::fs::PermissionsExt;
        let directory = scratch_directory();
        let path = directory.join("state.db-wal");
        fs::write(&path, b"frames").unwrap();
        let seen = stamp(&path).unwrap().unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        // Privileged users read through the mode; nothing to prove for them.
        if fs::File::open(&path).is_err() {
            assert!(matches!(read_image(&path, &seen), Err(Error::Io(_))));
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::remove_dir_all(directory).unwrap();
    }
}
