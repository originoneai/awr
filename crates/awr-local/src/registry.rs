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

const MAX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PROJECTS: usize = 10_000;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectState {
    #[default]
    Connected,
    Discovered,
    Attention,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalProject {
    pub key: String,
    pub project_id: Option<String>,
    pub name: String,
    pub root: PathBuf,
    pub registered_at: i64,
    pub last_used_at: i64,
    #[serde(default = "available")]
    pub available: bool,
    #[serde(default)]
    pub state: ProjectState,
    #[serde(default)]
    pub last_seen_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<String>,
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
    default_state_dir()
}

pub fn default_state_dir() -> Result<PathBuf> {
    #[cfg(windows)]
    if let Some(path) = std::env::var_os("LOCALAPPDATA") {
        return Ok(PathBuf::from(path).join("AWR"));
    }
    if let Some(path) = std::env::var_os("XDG_STATE_HOME")
        && Path::new(&path).is_absolute()
    {
        return Ok(PathBuf::from(path).join("awr"));
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or_else(|| Error::InvalidInput("local user state directory is unavailable".into()))?;
    Ok(PathBuf::from(home).join(".local/state/awr"))
}

pub fn is_default_state_dir(dir: &Path) -> bool {
    default_state_dir().is_ok_and(|expected| {
        dir.canonicalize().unwrap_or_else(|_| dir.into())
            == expected.canonicalize().unwrap_or(expected)
    })
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
            version: 2,
            projects: Vec::new(),
        });
    }
    let mut value: Catalog = read_json(&path)?;
    if ![1, 2].contains(&value.version) || value.projects.len() > MAX_PROJECTS {
        return Err(Error::InvalidInput(
            "unsupported local project catalog".into(),
        ));
    }
    value.version = 2;
    Ok(value)
}

/// Inspect only project identity metadata. A discovered directory receives no invented AWR identity.
pub fn inspect(root: &Path) -> Result<LocalProject> {
    let root = root.canonicalize()?;
    if !root.is_dir() {
        return Err(Error::InvalidInput(
            "project root must be a directory".into(),
        ));
    }
    let now = now_millis()?;
    let mut entry = LocalProject {
        key: String::new(),
        project_id: None,
        name: root
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        root,
        registered_at: now,
        last_used_at: 0,
        available: true,
        state: ProjectState::Discovered,
        last_seen_at: now,
        issue: None,
    };
    if entry.root.join(".awr").exists() {
        match Store::open_readonly(&entry.root.join(".awr/state.db"))
            .and_then(|store| store.project_by_root(&entry.root))
        {
            Ok(project) => {
                entry.project_id = Some(project.id.to_string());
                entry.name = project.name;
                entry.state = ProjectState::Connected;
            }
            Err(error) => {
                entry.state = ProjectState::Attention;
                entry.issue = Some(error.report().message);
            }
        }
    }
    let mut identity = Sha256::new();
    identity.update(entry.root.to_string_lossy().as_bytes());
    identity.update(b"\0");
    identity.update(entry.project_id.as_deref().unwrap_or("").as_bytes());
    entry.key = format!("{:x}", identity.finalize());
    Ok(entry)
}

fn merge(catalog: &mut Catalog, mut entry: LocalProject, used: bool) -> Result<LocalProject> {
    if entry.project_id.is_none()
        && let Some(old) = catalog
            .projects
            .iter_mut()
            .find(|project| project.root == entry.root && project.project_id.is_some())
    {
        // A scan may have observed the directory before a concurrent initialization finished.
        // A bound identity also survives missing/corrupt runtime files until explicit repair.
        old.last_seen_at = old.last_seen_at.max(entry.last_seen_at);
        old.available = validate(old).is_ok();
        if !old.available {
            old.state = ProjectState::Attention;
            old.issue = entry
                .issue
                .or_else(|| Some("registered AWR identity requires repair".into()));
        }
        if used {
            old.last_used_at = entry.last_seen_at;
        }
        return Ok(old.clone());
    }
    let old = catalog.projects.iter_mut().find(|project| {
        (project.root == entry.root && project.project_id == entry.project_id)
            || (project.root == entry.root && project.project_id.is_none())
    });
    if let Some(old) = old {
        entry.key = old.key.clone();
        entry.registered_at = old.registered_at;
        entry.last_used_at = if used {
            entry.last_seen_at
        } else {
            old.last_used_at
        };
        *old = entry.clone();
    } else {
        if catalog.projects.len() >= MAX_PROJECTS {
            return Err(Error::InvalidInput("local project catalog is full".into()));
        }
        if used {
            entry.last_used_at = entry.last_seen_at;
        }
        catalog.projects.push(entry.clone());
    }
    Ok(entry)
}

pub(crate) fn record(
    dir: &Path,
    entries: Vec<LocalProject>,
    used: bool,
) -> Result<Vec<LocalProject>> {
    let _lock = lock(dir, "projects.lock", Duration::from_secs(2))?;
    let mut catalog = catalog(dir)?;
    let merged = entries
        .into_iter()
        .map(|entry| merge(&mut catalog, entry, used))
        .collect::<Result<Vec<_>>>()?;
    write_json(&dir.join("projects.json"), &catalog)?;
    Ok(merged)
}

pub fn observe(dir: &Path, root: &Path) -> Result<LocalProject> {
    Ok(record(dir, vec![inspect(root)?], true)?.remove(0))
}

pub fn register(dir: &Path, root: &Path) -> Result<LocalProject> {
    let entry = inspect(root)?;
    if entry.state != ProjectState::Connected {
        return Err(Error::NotFound(
            "initialized AWR project; use overview inspect for intake guidance".into(),
        ));
    }
    Ok(record(dir, vec![entry], true)?.remove(0))
}

pub(crate) fn validate(entry: &LocalProject) -> Result<()> {
    if entry.root.canonicalize()? != entry.root {
        return Err(Error::SourceConflict(
            "registered project directory changed".into(),
        ));
    }
    let expected = entry
        .project_id
        .as_deref()
        .ok_or_else(|| Error::NotFound("project awaits AWR intake".into()))?;
    let store = Store::open_readonly(&entry.root.join(".awr/state.db"))?;
    if store.project_by_root(&entry.root)?.id.to_string() != expected {
        return Err(Error::SourceConflict(
            "registered project identity changed".into(),
        ));
    }
    Ok(())
}

pub fn list(dir: &Path) -> Result<Vec<LocalProject>> {
    let mut entries = catalog(dir)?.projects;
    for entry in &mut entries {
        entry.available = if entry.project_id.is_some() {
            validate(entry).is_ok()
        } else {
            entry
                .root
                .canonicalize()
                .is_ok_and(|root| root == entry.root && root.is_dir())
        };
    }
    entries.sort_by(|a, b| {
        b.last_used_at
            .cmp(&a.last_used_at)
            .then(b.registered_at.cmp(&a.registered_at))
            .then(a.root.cmp(&b.root))
    });
    Ok(entries)
}

pub(crate) fn select(dir: &Path, key: &str) -> Result<LocalProject> {
    let entry = get(dir, key)?;
    validate(&entry)?;
    Ok(entry)
}

pub fn get(dir: &Path, key: &str) -> Result<LocalProject> {
    catalog(dir)?
        .projects
        .into_iter()
        .find(|p| p.key == key)
        .ok_or_else(|| Error::NotFound("registered local project".into()))
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
