use awr_core::{Error, Result, now_millis};
use awr_store::Store;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const MAX_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalProject {
    pub key: String,
    pub project_id: String,
    pub name: String,
    pub root: PathBuf,
    pub registered_at: i64,
    pub last_used_at: i64,
    #[serde(default = "available")]
    pub available: bool,
}

fn available() -> bool {
    true
}

#[derive(Serialize, Deserialize)]
struct Catalog {
    version: u32,
    projects: Vec<LocalProject>,
}

pub fn state_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("AWR_LOCAL_HOME") {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err(Error::InvalidInput(
                "AWR_LOCAL_HOME must be absolute".into(),
            ));
        }
        return Ok(path);
    }
    #[cfg(windows)]
    if let Some(path) = std::env::var_os("LOCALAPPDATA") {
        return Ok(PathBuf::from(path).join("AWR"));
    }
    if let Some(path) = std::env::var_os("XDG_STATE_HOME") {
        if Path::new(&path).is_absolute() {
            return Ok(PathBuf::from(path).join("awr"));
        }
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or_else(|| Error::InvalidInput("local user state directory is unavailable".into()))?;
    Ok(PathBuf::from(home).join(".local/state/awr"))
}

pub(crate) fn prepare(dir: &Path) -> Result<()> {
    let mut options = std::fs::DirBuilder::new();
    options.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        options.mode(0o700);
    }
    options.create(dir)?;
    let meta = std::fs::symlink_metadata(dir)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(Error::InvalidInput(
            "local state must be a real directory".into(),
        ));
    }
    Ok(())
}

pub(crate) fn private_file(path: &Path) -> Result<File> {
    if std::fs::symlink_metadata(path).is_ok_and(|m| !m.is_file() || m.file_type().is_symlink()) {
        return Err(Error::InvalidInput(
            "local state requires regular files".into(),
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

pub(crate) fn lock(dir: &Path, name: &str, timeout: Duration) -> Result<File> {
    prepare(dir)?;
    let file = private_file(&dir.join(name))?;
    let start = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) if start.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(Error::SourceConflict(
                    "local service is busy; retry shortly".into(),
                ));
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
    }
}

pub(crate) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(Error::InvalidInput(
            "local state exceeds its size limit".into(),
        ));
    }
    Ok(serde_json::from_slice(&bytes)?)
}

pub(crate) fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let temp = path.with_extension(format!("{}.tmp", awr_core::Id::new()));
    let result = (|| {
        let mut file = private_file(&temp)?;
        file.write_all(&serde_json::to_vec_pretty(value)?)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}

fn catalog(dir: &Path) -> Result<Catalog> {
    let path = dir.join("projects.json");
    if !path.exists() {
        return Ok(Catalog {
            version: 1,
            projects: Vec::new(),
        });
    }
    let value: Catalog = read_json(&path)?;
    if value.version != 1 || value.projects.len() > 1000 {
        return Err(Error::InvalidInput(
            "unsupported local project catalog".into(),
        ));
    }
    Ok(value)
}

pub fn register(dir: &Path, root: &Path) -> Result<LocalProject> {
    let root = root.canonicalize()?;
    let store = Store::open_readonly(&root.join(".awr/state.db"))?;
    let project = store.project_by_root(&root)?;
    let mut identity = Sha256::new();
    identity.update(root.to_string_lossy().as_bytes());
    identity.update(b"\0");
    identity.update(project.id.to_string().as_bytes());
    let key = format!("{:x}", identity.finalize());
    let now = now_millis()?;
    let _lock = lock(dir, "projects.lock", Duration::from_secs(2))?;
    let mut catalog = catalog(dir)?;
    let entry = LocalProject {
        key: key.clone(),
        project_id: project.id.to_string(),
        name: project.name,
        root,
        registered_at: now,
        last_used_at: now,
        available: true,
    };
    let entry = if let Some(old) = catalog.projects.iter_mut().find(|p| p.key == key) {
        if old.project_id != entry.project_id {
            return Err(Error::SourceConflict("registered project identity changed; inspect the directory before registering it again".into()));
        }
        let entry = LocalProject {
            registered_at: old.registered_at,
            ..entry
        };
        *old = entry.clone();
        entry
    } else {
        if catalog.projects.len() >= 1000 {
            return Err(Error::InvalidInput("local project catalog is full".into()));
        }
        catalog.projects.push(entry.clone());
        entry
    };
    write_json(&dir.join("projects.json"), &catalog)?;
    Ok(entry)
}

pub(crate) fn validate(entry: &LocalProject) -> Result<()> {
    if entry.root.canonicalize()? != entry.root {
        return Err(Error::SourceConflict(
            "registered project directory changed".into(),
        ));
    }
    let store = Store::open_readonly(&entry.root.join(".awr/state.db"))?;
    if store.project_by_root(&entry.root)?.id.to_string() != entry.project_id {
        return Err(Error::SourceConflict(
            "registered project identity changed".into(),
        ));
    }
    Ok(())
}

pub fn list(dir: &Path) -> Result<Vec<LocalProject>> {
    let mut entries = catalog(dir)?.projects;
    for entry in &mut entries {
        entry.available = validate(entry).is_ok();
    }
    entries.sort_by(|a, b| {
        b.last_used_at
            .cmp(&a.last_used_at)
            .then(a.root.cmp(&b.root))
    });
    Ok(entries)
}

pub(crate) fn select(dir: &Path, key: &str) -> Result<LocalProject> {
    let entry = catalog(dir)?
        .projects
        .into_iter()
        .find(|p| p.key == key)
        .ok_or_else(|| Error::NotFound("registered local project".into()))?;
    validate(&entry)?;
    Ok(entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("awr-local-{}", awr_core::Id::new()));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn project(&self, name: &str) -> PathBuf {
            let root = self.0.join(name);
            std::fs::create_dir_all(root.join(".awr")).unwrap();
            Store::open(&root.join(".awr/state.db"))
                .unwrap()
                .register_project(&root, name, name)
                .unwrap();
            root
        }
        fn dir(&self) -> PathBuf {
            self.0.join("user-state")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn initialized_projects_are_deduplicated_and_latest_use_sorts_first() {
        let fixture = Fixture::new();
        let a = fixture.project("alpha");
        let b = fixture.project("beta");
        let first = register(&fixture.dir(), &a).unwrap();
        std::thread::sleep(Duration::from_millis(3));
        register(&fixture.dir(), &b).unwrap();
        assert_eq!(list(&fixture.dir()).unwrap()[0].name, "beta");
        std::thread::sleep(Duration::from_millis(3));
        let used = register(&fixture.dir(), &a.join(".")).unwrap();
        let entries = list(&fixture.dir()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].key, first.key);
        assert_eq!(used.registered_at, first.registered_at);
        assert!(entries.iter().all(|p| p.available));
    }

    #[test]
    fn a_plain_directory_is_never_initialized_by_registration() {
        let fixture = Fixture::new();
        assert!(register(&fixture.dir(), &fixture.0).is_err());
        assert!(!fixture.0.join(".awr").exists());
        assert!(list(&fixture.dir()).unwrap().is_empty());
    }

    #[test]
    fn moved_and_replaced_projects_remain_visible_and_cannot_be_selected() {
        let fixture = Fixture::new();
        let root = fixture.project("alpha");
        let entry = register(&fixture.dir(), &root).unwrap();
        std::fs::rename(&root, fixture.0.join("moved")).unwrap();
        assert!(!list(&fixture.dir()).unwrap()[0].available);
        assert!(select(&fixture.dir(), &entry.key).is_err());
        fixture.project("alpha");
        let replacement = register(&fixture.dir(), &root).unwrap();
        assert_ne!(replacement.key, entry.key);
        let entries = list(&fixture.dir()).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(
            entries
                .iter()
                .any(|p| p.key == replacement.key && p.available)
        );
        assert!(entries.iter().any(|p| p.key == entry.key && !p.available));
        assert!(select(&fixture.dir(), &entry.key).is_err());
    }

    #[test]
    fn concurrent_registrations_keep_every_project() {
        let fixture = Fixture::new();
        let jobs: Vec<_> = (0..12)
            .map(|i| {
                let root = fixture.project(&format!("project-{i}"));
                let dir = fixture.dir();
                std::thread::spawn(move || register(&dir, &root).unwrap())
            })
            .collect();
        for job in jobs {
            job.join().unwrap();
        }
        assert_eq!(list(&fixture.dir()).unwrap().len(), 12);
    }

    #[test]
    fn corrupt_catalogs_are_reported_and_preserved() {
        let fixture = Fixture::new();
        let root = fixture.project("alpha");
        prepare(&fixture.dir()).unwrap();
        let path = fixture.dir().join("projects.json");
        std::fs::write(&path, b"broken catalog").unwrap();
        assert!(register(&fixture.dir(), &root).is_err());
        assert!(list(&fixture.dir()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"broken catalog");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_aliases_share_one_entry_and_new_state_is_private() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let fixture = Fixture::new();
        let root = fixture.project("alpha");
        let alias = fixture.0.join("alias");
        symlink(&root, &alias).unwrap();
        assert_eq!(
            register(&fixture.dir(), &root).unwrap().key,
            register(&fixture.dir(), &alias).unwrap().key
        );
        assert_eq!(list(&fixture.dir()).unwrap().len(), 1);
        assert_eq!(
            std::fs::metadata(fixture.dir())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(fixture.dir().join("projects.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
