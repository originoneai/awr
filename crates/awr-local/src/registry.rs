use awr_core::{Error, Result, now_millis};
use awr_store::Store;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
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
    #[serde(default)]
    pub intake_requested: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<String>,
}

fn available() -> bool {
    true
}

pub(crate) fn is_temporary_session_directory(root: &Path) -> bool {
    root.ancestors().any(|ancestor| {
        ancestor
            .parent()
            .is_some_and(|parent| parent.ends_with("Codex"))
            && ancestor
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.len() == 10
                        && name.bytes().enumerate().all(|(index, byte)| match index {
                            4 | 7 => byte == b'-',
                            _ => byte.is_ascii_digit(),
                        })
                })
    })
}

#[derive(Serialize, Deserialize)]
struct Catalog {
    version: u32,
    projects: Vec<LocalProject>,
}

#[derive(Serialize, Deserialize)]
struct HiddenProjects {
    version: u32,
    keys: BTreeSet<String>,
}

fn hidden_projects(dir: &Path) -> Result<HiddenProjects> {
    let path = dir.join("hidden-projects.json");
    if !path.exists() {
        return Ok(HiddenProjects {
            version: 1,
            keys: BTreeSet::new(),
        });
    }
    let hidden: HiddenProjects = read_json(&path)?;
    if hidden.version != 1 || hidden.keys.len() > MAX_PROJECTS {
        return Err(Error::InvalidInput(
            "unsupported hidden project catalog".into(),
        ));
    }
    Ok(hidden)
}

pub(crate) fn hide(dir: &Path, key: &str) -> Result<()> {
    let _lock = lock(dir, "projects.lock", Duration::from_secs(2))?;
    let _entry = get(dir, key)?;
    let mut hidden = hidden_projects(dir)?;
    hidden.keys.insert(key.to_owned());
    write_json(&dir.join("hidden-projects.json"), &hidden)
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
            version: 3,
            projects: Vec::new(),
        });
    }
    let mut value: Catalog = read_json(&path)?;
    if ![1, 2, 3].contains(&value.version) || value.projects.len() > MAX_PROJECTS {
        return Err(Error::InvalidInput(
            "unsupported local project catalog".into(),
        ));
    }
    value.version = 3;
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
    if is_temporary_session_directory(&root) {
        return Err(Error::InvalidInput(
            "Temporary agent session directories are excluded from Project Hub. Use a durable project directory.".into(),
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
        intake_requested: false,
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
        old.intake_requested |= entry.intake_requested;
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
        entry.intake_requested |= old.intake_requested;
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
    let mut hidden = hidden_projects(dir)?;
    let mut restored = false;
    let merged = entries
        .into_iter()
        .map(|entry| {
            let explicit = entry.intake_requested;
            let project = merge(&mut catalog, entry, used)?;
            if explicit {
                restored |= hidden.keys.remove(&project.key);
            }
            Ok(project)
        })
        .collect::<Result<Vec<_>>>()?;
    write_json(&dir.join("projects.json"), &catalog)?;
    if restored {
        write_json(&dir.join("hidden-projects.json"), &hidden)?;
    }
    Ok(merged)
}

pub fn observe(dir: &Path, root: &Path) -> Result<LocalProject> {
    let entry = inspect(root)?;
    if entry.state == ProjectState::Discovered
        && !catalog(dir)?
            .projects
            .iter()
            .any(|old| old.root == entry.root && (old.intake_requested || old.project_id.is_some()))
    {
        return Err(Error::NotFound(
            "project has not been enrolled for AWR intake".into(),
        ));
    }
    Ok(record(dir, vec![entry], true)?.remove(0))
}

/// Refresh the usage time of a project that is already in the catalog.
///
/// A project that is not registered is left alone and `None` is returned: ordinary commands
/// must not grow the catalog, because a machine that runs throwaway fixtures, copies and agent
/// scratch directories would collect all of them. Registration is explicit (`register`).
pub fn touch(dir: &Path, root: &Path) -> Result<Option<LocalProject>> {
    if !dir.exists() {
        return Ok(None);
    }
    let entry = inspect(root)?;
    let _lock = lock(dir, "projects.lock", Duration::from_secs(2))?;
    let mut catalog = catalog(dir)?;
    if !catalog.projects.iter().any(|old| old.root == entry.root) {
        return Ok(None);
    }
    let project = merge(&mut catalog, entry, true)?;
    write_json(&dir.join("projects.json"), &catalog)?;
    Ok(Some(project))
}

pub(crate) fn request_intake(dir: &Path, root: &Path) -> Result<LocalProject> {
    if !root.is_absolute() {
        return Err(Error::InvalidInput(
            "project root must be an absolute directory path".into(),
        ));
    }
    let mut entry = inspect(root)?;
    entry.intake_requested = true;
    Ok(record(dir, vec![entry], false)?.remove(0))
}

/// Retire unused discovery suggestions only after a complete scan proves they are not project roots.
pub(crate) fn retire_candidates(
    dir: &Path,
    visited: &BTreeSet<PathBuf>,
    selected: &BTreeSet<PathBuf>,
    excluded: &[PathBuf],
) -> Result<usize> {
    let _lock = lock(dir, "projects.lock", Duration::from_secs(2))?;
    let mut catalog = catalog(dir)?;
    let before = catalog.projects.len();
    catalog.projects.retain(|entry| {
        let is_excluded = excluded.iter().any(|path| entry.root.starts_with(path));
        entry.project_id.is_some()
            || entry.last_used_at != 0
            || entry.intake_requested
            || selected.contains(&entry.root)
            || (!is_excluded
                && (entry.state == ProjectState::Attention || !visited.contains(&entry.root)))
    });
    let retired = before - catalog.projects.len();
    if retired != 0 {
        write_json(&dir.join("projects.json"), &catalog)?;
    }
    Ok(retired)
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
    if is_temporary_session_directory(&entry.root) {
        return Err(Error::InvalidInput(
            "Temporary agent session directories are excluded from Project Hub.".into(),
        ));
    }
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
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let (mut entries, hidden) = {
        let _lock = lock(dir, "projects.lock", Duration::from_secs(2))?;
        (catalog(dir)?.projects, hidden_projects(dir)?)
    };
    entries.retain(|entry| {
        !hidden.keys.contains(&entry.key) && !is_temporary_session_directory(&entry.root)
    });
    entries.retain(|entry| {
        entry.project_id.is_some()
            || entry.intake_requested
            || entry.state != ProjectState::Discovered
    });
    for entry in &mut entries {
        entry.available = if entry.project_id.is_some() {
            validate(entry).is_ok()
        } else {
            entry
                .root
                .canonicalize()
                .is_ok_and(|root| root == entry.root && root.is_dir())
        };
        if !entry.available
            && let Ok(current) = inspect(&entry.root)
            && current.root == entry.root
            && current.project_id.is_none()
        {
            entry.available = true;
            entry.state = ProjectState::Attention;
            entry.issue = current
                .issue
                .or_else(|| Some("registered AWR identity requires repair".into()));
        }
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
    fn touch_refreshes_a_registered_project_and_never_adds_one() {
        let fixture = Fixture::new();
        let registered = fixture.project("alpha");
        let stranger = fixture.project("beta");
        assert!(
            touch(&fixture.dir(), &registered).unwrap().is_none(),
            "no catalog yet: nothing to refresh, and no directory is created"
        );
        assert!(!fixture.dir().exists());
        let first = register(&fixture.dir(), &registered).unwrap();
        std::thread::sleep(Duration::from_millis(3));
        assert!(touch(&fixture.dir(), &stranger).unwrap().is_none());
        assert_eq!(list(&fixture.dir()).unwrap().len(), 1);
        let touched = touch(&fixture.dir(), &registered).unwrap().unwrap();
        assert_eq!(touched.key, first.key);
        assert_eq!(touched.registered_at, first.registered_at);
        assert!(touched.last_used_at > first.last_used_at);
        assert_eq!(list(&fixture.dir()).unwrap().len(), 1);
    }

    #[test]
    fn dated_session_paths_exclude_descendants_without_excluding_durable_workspaces() {
        for path in [
            "workspace/Codex/2026-09-29",
            "workspace/Codex/2026-09-29/chat/work/project",
        ] {
            assert!(is_temporary_session_directory(Path::new(path)), "{path}");
        }
        for path in [
            "workspace/Codex",
            "workspace/Codex/permanent-project",
            "workspace/projects/2026-09-29",
            "workspace/Codex/2026-09",
            "workspace/Codex/202a-09-29",
        ] {
            assert!(!is_temporary_session_directory(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn temporary_projects_are_filtered_from_prior_catalogs_and_cannot_be_enrolled() {
        let fixture = Fixture::new();
        let durable = fixture.project("durable");
        let temporary = fixture.project("Codex/2026-09-29/chat/work/project");
        let mut prior = register(&fixture.dir(), &durable).unwrap();
        let store = Store::open_readonly(&temporary.join(".awr/state.db")).unwrap();
        let project = store
            .project_by_root(&temporary.canonicalize().unwrap())
            .unwrap();
        prior.root = temporary.clone();
        prior.key = "f".repeat(64);
        prior.project_id = Some(project.id.to_string());
        record(&fixture.dir(), vec![prior.clone()], false).unwrap();
        assert_eq!(catalog(&fixture.dir()).unwrap().projects.len(), 2);
        assert_eq!(list(&fixture.dir()).unwrap().len(), 1);
        assert!(select(&fixture.dir(), &prior.key).is_err());
        assert!(hide(&fixture.dir(), &prior.key).is_ok());
        assert!(register(&fixture.dir(), &temporary).is_err());
        assert!(observe(&fixture.dir(), &temporary).is_err());
        assert!(request_intake(&fixture.dir(), &temporary).is_err());
        assert_eq!(catalog(&fixture.dir()).unwrap().projects.len(), 2);
        assert!(temporary.join(".awr/state.db").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn a_canonical_alias_cannot_enroll_a_temporary_session_project() {
        let fixture = Fixture::new();
        let temporary = fixture.project("Codex/2026-09-29/project");
        let alias = fixture.0.join("alias");
        std::os::unix::fs::symlink(&temporary, &alias).unwrap();
        assert!(request_intake(&fixture.dir(), &alias).is_err());
        assert!(list(&fixture.dir()).unwrap().is_empty());
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
    fn explicit_intake_validates_paths_deduplicates_and_preserves_directory_contents() {
        let fixture = Fixture::new();
        let root = fixture.0.join("pending project");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("README.md"), "Existing project sources\n").unwrap();
        assert!(observe(&fixture.dir(), &root).is_err());
        assert!(list(&fixture.dir()).unwrap().is_empty());
        assert!(request_intake(&fixture.dir(), Path::new("relative/path")).is_err());
        assert!(request_intake(&fixture.dir(), &root.join("missing")).is_err());
        let pending = request_intake(&fixture.dir(), &root).unwrap();
        let repeated = request_intake(&fixture.dir(), &root.join(".")).unwrap();
        assert_eq!(pending.key, repeated.key);
        assert_eq!(pending.state, ProjectState::Discovered);
        assert!(pending.intake_requested);
        assert_eq!(pending.last_used_at, 0);
        assert_eq!(list(&fixture.dir()).unwrap().len(), 1);
        assert_eq!(
            std::fs::read_to_string(root.join("README.md")).unwrap(),
            "Existing project sources\n"
        );
        assert!(!root.join(".awr").exists());
    }

    #[test]
    fn damaged_runtime_keeps_its_identity_and_an_accessible_repair_card() {
        let fixture = Fixture::new();
        let root = fixture.project("alpha");
        let original = register(&fixture.dir(), &root).unwrap();
        std::fs::remove_dir_all(root.join(".awr")).unwrap();
        observe(&fixture.dir(), &root).unwrap();
        let entry = list(&fixture.dir()).unwrap().remove(0);
        assert_eq!(entry.key, original.key);
        assert_eq!(entry.project_id, original.project_id);
        assert_eq!(entry.state, ProjectState::Attention);
        assert!(entry.available);
        assert!(entry.issue.is_some());
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

    #[test]
    fn hidden_display_preferences_survive_concurrent_registration_and_pending_adoption() {
        let fixture = Fixture::new();
        let managed = fixture.project("managed");
        let first = register(&fixture.dir(), &managed).unwrap();
        let pending = fixture.0.join("pending");
        std::fs::create_dir(&pending).unwrap();
        let second = request_intake(&fixture.dir(), &pending).unwrap();
        let bytes = std::fs::read(managed.join(".awr/state.db")).unwrap();
        let dir = fixture.dir();
        let used = managed.clone();
        let registration = std::thread::spawn(move || {
            for _ in 0..8 {
                register(&dir, &used).unwrap();
            }
        });
        hide(&fixture.dir(), &first.key).unwrap();
        hide(&fixture.dir(), &second.key).unwrap();
        registration.join().unwrap();
        hide(&fixture.dir(), &first.key).unwrap();
        assert!(list(&fixture.dir()).unwrap().is_empty());
        assert_eq!(std::fs::read(managed.join(".awr/state.db")).unwrap(), bytes);
        assert!(!pending.join(".awr").exists());
        fixture.project("pending");
        assert_eq!(register(&fixture.dir(), &pending).unwrap().key, second.key);
        assert!(list(&fixture.dir()).unwrap().is_empty());
        assert_eq!(
            request_intake(&fixture.dir(), &pending).unwrap().key,
            second.key
        );
        assert_eq!(list(&fixture.dir()).unwrap().len(), 1);
        assert_eq!(hidden_projects(&fixture.dir()).unwrap().keys.len(), 1);
    }

    #[test]
    fn invalid_hidden_preferences_are_preserved_and_never_silently_reveal_cards() {
        let fixture = Fixture::new();
        let root = fixture.project("managed");
        let entry = register(&fixture.dir(), &root).unwrap();
        let path = fixture.dir().join("hidden-projects.json");
        std::fs::write(&path, b"broken display preferences").unwrap();
        assert!(list(&fixture.dir()).is_err());
        assert!(register(&fixture.dir(), &root).is_err());
        assert!(hide(&fixture.dir(), &entry.key).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"broken display preferences");
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
