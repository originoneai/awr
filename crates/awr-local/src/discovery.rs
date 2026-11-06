//! Metadata-only project discovery. All durable writes belong to the per-user catalog.
use crate::registry::{self, LocalProject, ProjectState};
use awr_core::{Error, Result, now_millis};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, VecDeque},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

const ISSUE_LIMIT: usize = 100;
const BATCH_SIZE: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DiscoveryConfig {
    pub version: u32,
    pub roots: Vec<PathBuf>,
    pub exclude_names: Vec<String>,
    #[serde(skip_serializing)]
    pub interval_seconds: u64,
    pub directory_limit: usize,
    pub depth_limit: usize,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        let roots = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .into_iter()
            .collect();
        Self {
            version: 1,
            roots,
            exclude_names: [
                ".*",
                "node_modules",
                "target",
                "vendor",
                "dist",
                "build",
                "out",
                "__pycache__",
                "venv",
                "env",
                "site-packages",
                "Library",
                "AppData",
                "miniconda3",
                "anaconda3",
                "tmp",
                "temp",
                "fixtures",
                "test-fixtures",
                "fixture-*",
                "*-fixture-*",
                "artifacts",
                "outputs",
                "coverage",
                "*.app",
                "evidence",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            interval_seconds: 60,
            directory_limit: 200_000,
            depth_limit: 32,
        }
    }
}

impl DiscoveryConfig {
    fn validate(&self) -> Result<()> {
        if self.version != 1
            || self.roots.len() > 128
            || self.roots.iter().any(|root| !root.is_absolute())
            || !(1..=3600).contains(&self.interval_seconds)
            || !(1..=1_000_000).contains(&self.directory_limit)
            || !(1..=128).contains(&self.depth_limit)
            || self.exclude_names.len() > 128
            || self.exclude_names.iter().any(|pattern| {
                pattern.is_empty() || pattern.len() > 200 || pattern.contains(['/', '\\'])
            })
        {
            return Err(Error::InvalidInput(
                "invalid local discovery configuration".into(),
            ));
        }
        exclusions(self)?;
        Ok(())
    }
}

pub fn config(dir: &Path) -> Result<DiscoveryConfig> {
    let path = dir.join("discovery.json");
    let value = if path.exists() {
        registry::read_json(&path)?
    } else {
        let mut defaults = DiscoveryConfig::default();
        if !registry::is_default_state_dir(dir) {
            defaults.roots.clear();
        }
        defaults
    };
    value.validate()?;
    Ok(value)
}

pub fn configure(dir: &Path, mut value: DiscoveryConfig) -> Result<DiscoveryConfig> {
    value.validate()?;
    value.roots = value
        .roots
        .into_iter()
        .map(|root| root.canonicalize().unwrap_or(root))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let _lock = registry::lock(dir, "discovery-config.lock", Duration::from_secs(2))?;
    registry::write_json(&dir.join("discovery.json"), &value)?;
    Ok(value)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanIssue {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryReport {
    pub version: u32,
    pub state: String,
    pub roots: Vec<PathBuf>,
    pub exclude_names: Vec<String>,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub last_completed_at: Option<i64>,
    pub directories_scanned: usize,
    pub projects_found: usize,
    pub connected: usize,
    pub pending: usize,
    pub attention: usize,
    pub excluded_directories: usize,
    #[serde(default)]
    pub dependency_caches_skipped: usize,
    #[serde(default)]
    pub retired_candidates: usize,
    pub symlinks_skipped: usize,
    pub limit_reached: bool,
    pub issues: Vec<ScanIssue>,
    pub omitted_issues: usize,
}

impl DiscoveryReport {
    fn new(config: &DiscoveryConfig) -> Self {
        Self {
            version: 1,
            state: "pending".into(),
            roots: config.roots.clone(),
            exclude_names: config.exclude_names.clone(),
            started_at: None,
            completed_at: None,
            last_completed_at: None,
            directories_scanned: 0,
            projects_found: 0,
            connected: 0,
            pending: 0,
            attention: 0,
            excluded_directories: 0,
            dependency_caches_skipped: 0,
            retired_candidates: 0,
            symlinks_skipped: 0,
            limit_reached: false,
            issues: Vec::new(),
            omitted_issues: 0,
        }
    }
    fn issue(&mut self, path: &Path, reason: impl Into<String>) {
        if self.issues.len() < ISSUE_LIMIT {
            self.issues.push(ScanIssue {
                path: path.into(),
                reason: reason.into(),
            });
        } else {
            self.omitted_issues += 1;
        }
    }
}

pub fn discovery_status(dir: &Path) -> Result<DiscoveryReport> {
    let configured = config(dir)?;
    let path = dir.join("discovery-status.json");
    if path.exists() {
        registry::read_json(&path)
    } else {
        Ok(DiscoveryReport::new(&configured))
    }
}

fn exclusions(config: &DiscoveryConfig) -> Result<globset::GlobSet> {
    let mut builder = globset::GlobSetBuilder::new();
    for pattern in &config.exclude_names {
        builder.add(
            globset::Glob::new(pattern)
                .map_err(|_| Error::InvalidInput("invalid discovery exclusion pattern".into()))?,
        );
    }
    builder
        .build()
        .map_err(|_| Error::InvalidInput("invalid discovery exclusion set".into()))
}

fn flush(dir: &Path, batch: &mut Vec<LocalProject>, report: &DiscoveryReport) -> Result<()> {
    if !batch.is_empty() {
        registry::record(dir, std::mem::take(batch), false)?;
    }
    registry::write_json(&dir.join("discovery-status.json"), report)
}

pub fn discover(dir: &Path) -> Result<DiscoveryReport> {
    scan(dir, &AtomicBool::new(false))
}

pub(crate) fn scan(dir: &Path, cancelled: &AtomicBool) -> Result<DiscoveryReport> {
    let _lock = registry::lock(dir, "discovery.lock", Duration::ZERO)?;
    let configured = config(dir)?;
    let excluded_names = exclusions(&configured)?;
    let mut report = DiscoveryReport::new(&configured);
    if let Ok(previous) = discovery_status(dir) {
        report.last_completed_at = previous.last_completed_at;
    }
    report.started_at = Some(now_millis()?);
    report.state = "scanning".into();
    registry::write_json(&dir.join("discovery-status.json"), &report)?;
    let mut queue: VecDeque<_> = configured
        .roots
        .iter()
        .map(|root| (root.clone(), 0))
        .collect();
    let mut visited = BTreeSet::new();
    let mut selected = BTreeSet::new();
    let mut excluded_paths = Vec::new();
    let mut batch = Vec::new();
    let state_root = dir.canonicalize()?;
    while let Some((path, depth)) = queue.pop_front() {
        if cancelled.load(Ordering::Relaxed) {
            report.state = "interrupted".into();
            break;
        }
        if report.directories_scanned >= configured.directory_limit {
            report.limit_reached = true;
            report.issue(
                &path,
                "directory limit reached; increase the discovery limit or narrow scan roots",
            );
            break;
        }
        let root = match path.canonicalize() {
            Ok(root) => root,
            Err(error) => {
                report.issue(&path, error.to_string());
                continue;
            }
        };
        if root.starts_with(&state_root) || !visited.insert(root.clone()) {
            continue;
        }
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) => {
                report.issue(&root, error.to_string());
                continue;
            }
        };
        report.directories_scanned += 1;
        let candidate =
            root.join(".awr/project.toml").is_file() || root.join(".awr/state.db").is_file();
        if candidate {
            match registry::inspect(&root) {
                Ok(project) => {
                    selected.insert(root.clone());
                    report.projects_found += 1;
                    match project.state {
                        ProjectState::Connected => report.connected += 1,
                        ProjectState::Discovered => report.pending += 1,
                        ProjectState::Attention => report.attention += 1,
                    }
                    batch.push(project);
                }
                Err(error) => report.issue(&root, error.report().message),
            }
        }
        for entry in entries {
            if cancelled.load(Ordering::Relaxed) {
                break;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    report.issue(&root, error.to_string());
                    continue;
                }
            };
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(error) => {
                    report.issue(&entry.path(), error.to_string());
                    continue;
                }
            };
            if kind.is_symlink() {
                report.symlinks_skipped += 1;
                continue;
            }
            if !kind.is_dir() {
                continue;
            }
            let dependency_cache = entry.file_name() == "mod"
                && root.file_name().is_some_and(|name| name == "pkg")
                && entry.path().join("cache/download").is_dir();
            if excluded_names.is_match(entry.file_name()) || dependency_cache {
                report.excluded_directories += 1;
                report.dependency_caches_skipped += usize::from(dependency_cache);
                excluded_paths.push(entry.path());
            } else if depth >= configured.depth_limit {
                report.limit_reached = true;
                report.issue(&entry.path(), "depth limit reached");
            } else {
                if queue.len() + report.directories_scanned >= configured.directory_limit {
                    report.limit_reached = true;
                    report.issue(&entry.path(), "queued directory limit reached");
                    break;
                }
                queue.push_back((entry.path(), depth + 1));
            }
        }
        if batch.len() >= BATCH_SIZE || report.directories_scanned % 1000 == 0 {
            flush(dir, &mut batch, &report)?;
        }
    }
    if report.state != "interrupted" {
        report.state = if report.issues.is_empty() && !report.limit_reached {
            "complete"
        } else {
            "partial"
        }
        .into();
        report.completed_at = Some(now_millis()?);
        if report.state == "complete" {
            report.last_completed_at = report.completed_at;
        }
    }
    flush(dir, &mut batch, &report)?;
    if report.state == "complete" {
        report.retired_candidates =
            registry::retire_candidates(dir, &visited, &selected, &excluded_paths)?;
        registry::write_json(&dir.join("discovery-status.json"), &report)?;
    }
    Ok(report)
}

pub(crate) async fn discover_on_startup(
    dir: PathBuf,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let cancelled = std::sync::Arc::new(AtomicBool::new(false));
    let scan_cancelled = cancelled.clone();
    let scan_dir = dir.clone();
    let mut job = tokio::task::spawn_blocking(move || scan(&scan_dir, &scan_cancelled));
    tokio::select! {
        _ = shutdown.changed() => {
            cancelled.store(true, Ordering::Relaxed);
            let _ = job.await;
            return;
        }
        outcome = &mut job => {
            if let Ok(Err(error)) = outcome {
                if let Ok(mut report) = discovery_status(&dir) {
                    report.state = "error".into();
                    report.issue(&dir, error.report().message);
                    let _ = registry::write_json(&dir.join("discovery-status.json"), &report);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use awr_store::Store;

    struct Workspace(PathBuf);
    impl Workspace {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("awr-discovery-{}", awr_core::Id::new()));
            std::fs::create_dir(&root).unwrap();
            Self(root.canonicalize().unwrap())
        }
        fn directory(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::create_dir_all(&path).unwrap();
            path
        }
        fn project(&self, name: &str) -> PathBuf {
            let root = self.directory(name);
            std::fs::create_dir(root.join(".awr")).unwrap();
            Store::open(&root.join(".awr/state.db"))
                .unwrap()
                .register_project(&root, name, name)
                .unwrap();
            root
        }
        fn settings(&self) -> PathBuf {
            let dir = self.0.join("user-state");
            configure(
                &dir,
                DiscoveryConfig {
                    roots: vec![self.0.clone()],
                    ..Default::default()
                },
            )
            .unwrap();
            dir
        }
    }
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn discovers_only_existing_awr_projects_without_initializing_or_rewriting_them() {
        let f = Workspace::new();
        let connected = f.project("existing");
        let pending = f.directory("application");
        std::fs::create_dir(pending.join(".git")).unwrap();
        let ordinary = f.directory("notes");
        let invalid = f.directory("incomplete");
        std::fs::create_dir(invalid.join(".awr")).unwrap();
        std::fs::write(invalid.join(".awr/project.toml"), "").unwrap();
        let readme = connected.join("README.md");
        std::fs::write(&readme, "Original project requirements\n").unwrap();
        let database = std::fs::read(connected.join(".awr/state.db")).unwrap();
        let dir = f.settings();
        let report = discover(&dir).unwrap();
        assert_eq!(
            (
                report.projects_found,
                report.connected,
                report.pending,
                report.attention
            ),
            (2, 1, 0, 1)
        );
        assert_eq!(report.state, "complete");
        let projects = registry::list(&dir).unwrap();
        assert_eq!(projects.len(), 2);
        assert!(projects.iter().all(|entry| entry.last_used_at == 0));
        assert!(
            projects
                .iter()
                .find(|entry| entry.root == connected)
                .unwrap()
                .project_id
                .is_some()
        );
        assert!(!projects.iter().any(|entry| entry.root == pending));
        assert!(!pending.join(".awr").exists());
        assert!(!ordinary.join(".awr").exists());
        assert_eq!(
            std::fs::read_to_string(readme).unwrap(),
            "Original project requirements\n"
        );
        assert_eq!(
            std::fs::read(connected.join(".awr/state.db")).unwrap(),
            database
        );
    }

    #[test]
    fn package_subdirectories_and_dependency_caches_do_not_become_projects() {
        let f = Workspace::new();
        f.directory("repository/.git");
        let package = f.directory("repository/packages/library");
        std::fs::write(package.join("package.json"), "{}").unwrap();
        f.directory("repository/tools/nested-git/.git");
        let connected = f.project("repository/tools/nested-awr");
        let plain = f.directory("plain-project");
        std::fs::write(plain.join("Cargo.toml"), "[workspace]\n").unwrap();
        f.directory("go/pkg/mod/cache/download");
        let dependency = f.directory("go/pkg/mod/example/dependency");
        std::fs::write(dependency.join("go.mod"), "module example/dependency\n").unwrap();
        let application = f.directory("Installed.app/Contents");
        std::fs::write(application.join("package.json"), "{}").unwrap();
        let dir = f.settings();
        let report = discover(&dir).unwrap();
        assert_eq!(report.projects_found, 1);
        assert_eq!(report.dependency_caches_skipped, 1);
        let projects = registry::list(&dir).unwrap();
        assert!(projects.iter().any(|entry| entry.root == connected));
        assert!(!projects.iter().any(|entry| entry.root == plain));
        assert!(
            !projects
                .iter()
                .any(|entry| entry.root == package || entry.root == dependency)
        );
    }

    #[test]
    fn complete_scans_retire_only_unused_unbound_suggestions_and_preserve_moved_or_used_entries() {
        let f = Workspace::new();
        f.directory("repository/.git");
        let nested = f.directory("repository/packages/nested");
        std::fs::write(nested.join("package.json"), "{}").unwrap();
        f.directory("go/pkg/mod/cache/download");
        let dependency = f.directory("go/pkg/mod/example/dependency");
        std::fs::write(dependency.join("go.mod"), "module example/dependency\n").unwrap();
        let missing = f.directory("moved-project");
        let dir = f.settings();
        registry::record(
            &dir,
            [&nested, &dependency, &missing]
                .into_iter()
                .map(|root| registry::inspect(root).unwrap())
                .collect(),
            false,
        )
        .unwrap();
        registry::request_intake(&dir, &missing).unwrap();
        std::fs::rename(&missing, f.0.join("renamed-project")).unwrap();
        let used = f.directory("repository/packages/used");
        registry::request_intake(&dir, &used).unwrap();
        registry::observe(&dir, &used).unwrap();
        let bound = f.project("repository/evidence/bound-project");
        let registered = registry::register(&dir, &bound).unwrap();
        let report = discover(&dir).unwrap();
        assert_eq!(report.retired_candidates, 2);
        let projects = registry::list(&dir).unwrap();
        assert!(
            !projects
                .iter()
                .any(|entry| entry.root == nested || entry.root == dependency)
        );
        assert!(
            projects
                .iter()
                .any(|entry| entry.root == used && entry.last_used_at > 0)
        );
        assert!(
            projects
                .iter()
                .any(|entry| entry.key == registered.key
                    && entry.project_id == registered.project_id)
        );
        assert!(
            projects
                .iter()
                .any(|entry| entry.root == missing && !entry.available)
        );
    }

    #[test]
    fn rescans_preserve_usage_order_and_discover_additions_while_retaining_unavailable_projects() {
        let f = Workspace::new();
        let project = f.project("active");
        let dir = f.settings();
        let used = registry::register(&dir, &project).unwrap();
        discover(&dir).unwrap();
        let added = f.project("new-application");
        let report = discover(&dir).unwrap();
        assert_eq!(report.projects_found, 2);
        let projects = registry::list(&dir).unwrap();
        assert_eq!(projects[0].last_used_at, used.last_used_at);
        assert_eq!(projects[0].key, used.key);
        std::fs::rename(&added, f.0.join("renamed-application")).unwrap();
        discover(&dir).unwrap();
        let projects = registry::list(&dir).unwrap();
        assert_eq!(projects.len(), 3);
        assert!(
            projects
                .iter()
                .any(|entry| entry.root == added && !entry.available)
        );
    }

    #[test]
    fn adoption_keeps_the_discovered_card_key_and_then_binds_the_actual_project_identity() {
        let f = Workspace::new();
        let root = f.directory("application");
        std::fs::write(root.join("Cargo.toml"), "[package]\nname='application'\n").unwrap();
        let dir = f.settings();
        discover(&dir).unwrap();
        assert!(registry::list(&dir).unwrap().is_empty());
        let pending = registry::request_intake(&dir, &root).unwrap();
        discover(&dir).unwrap();
        assert_eq!(registry::list(&dir).unwrap().len(), 1);
        assert!(registry::select(&dir, &pending.key).is_err());
        f.project("application");
        let connected = registry::register(&dir, &root).unwrap();
        assert_eq!(connected.key, pending.key);
        assert!(connected.project_id.is_some());
        assert_eq!(registry::list(&dir).unwrap().len(), 1);
        assert!(registry::select(&dir, &pending.key).is_ok());
        discover(&dir).unwrap();
        let rescanned = registry::list(&dir).unwrap();
        assert_eq!(rescanned.len(), 1);
        assert_eq!(rescanned[0].key, pending.key);
    }

    #[test]
    fn prior_catalogs_remain_readable_and_migrate_explicitly_to_the_intake_aware_version() {
        let f = Workspace::new();
        let root = f.project("existing");
        let dir = f.settings();
        let entry = registry::register(&dir, &root).unwrap();
        for version in [1, 2] {
            let legacy = serde_json::json!({"version":version,"projects":[{
                "key":entry.key,"project_id":entry.project_id,"name":entry.name,"root":entry.root,
                "registered_at":entry.registered_at,"last_used_at":entry.last_used_at,"available":true
            }]});
            registry::write_json(&dir.join("projects.json"), &legacy).unwrap();
            discover(&dir).unwrap();
            assert_eq!(registry::list(&dir).unwrap()[0].key, entry.key);
            let catalog: serde_json::Value =
                registry::read_json(&dir.join("projects.json")).unwrap();
            assert_eq!(catalog["version"], 3);
            assert_eq!(catalog["projects"][0]["intake_requested"], false);
            assert_eq!(
                catalog["projects"][0]["project_id"],
                legacy["projects"][0]["project_id"]
            );
        }
    }

    #[test]
    fn exclusions_limits_missing_roots_and_cancellation_are_reported_without_false_full_coverage() {
        let f = Workspace::new();
        f.project("node_modules/dependency");
        f.project("fixtures/scenario");
        f.project("visible/project");
        let dir = f.settings();
        let report = discover(&dir).unwrap();
        assert_eq!(report.projects_found, 1);
        assert_eq!(
            report.excluded_directories, 3,
            "dependency and fixture directories plus the connected project's private runtime are excluded"
        );
        configure(
            &dir,
            DiscoveryConfig {
                roots: vec![f.0.clone()],
                directory_limit: 1,
                ..Default::default()
            },
        )
        .unwrap();
        let partial = discover(&dir).unwrap();
        assert_eq!(partial.state, "partial");
        assert!(partial.limit_reached);
        configure(
            &dir,
            DiscoveryConfig {
                roots: vec![f.0.join("missing")],
                ..Default::default()
            },
        )
        .unwrap();
        let missing = discover(&dir).unwrap();
        assert_eq!(missing.state, "partial");
        assert_eq!(missing.issues.len(), 1);
        let interrupted = scan(&dir, &AtomicBool::new(true)).unwrap();
        assert_eq!(interrupted.state, "interrupted");
        assert!(interrupted.completed_at.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn overlapping_roots_and_symlinks_neither_duplicate_projects_nor_escape_the_scan_scope() {
        let f = Workspace::new();
        let project = f.project("workspace/project");
        let outside = Workspace::new();
        outside.project("outside");
        std::os::unix::fs::symlink(&outside.0, f.0.join("outside-alias")).unwrap();
        std::os::unix::fs::symlink(&f.0, f.0.join("loop")).unwrap();
        let dir = f.settings();
        configure(
            &dir,
            DiscoveryConfig {
                roots: vec![f.0.clone(), project.clone()],
                ..Default::default()
            },
        )
        .unwrap();
        let report = discover(&dir).unwrap();
        assert_eq!(report.projects_found, 1);
        assert_eq!(report.symlinks_skipped, 2);
        assert_eq!(registry::list(&dir).unwrap().len(), 1);
    }

    #[test]
    fn isolated_namespaces_have_no_implicit_home_scan_and_invalid_settings_are_preserved() {
        let f = Workspace::new();
        let dir = f.0.join("isolated");
        assert!(config(&dir).unwrap().roots.is_empty());
        registry::prepare(&dir).unwrap();
        std::fs::write(dir.join("discovery.json"), "invalid settings").unwrap();
        assert!(discover(&dir).is_err());
        assert_eq!(
            std::fs::read_to_string(dir.join("discovery.json")).unwrap(),
            "invalid settings"
        );
    }

    #[test]
    fn stale_discovery_cannot_downgrade_a_concurrently_initialized_project() {
        let f = Workspace::new();
        let root = f.directory("application");
        let dir = f.settings();
        let stale = registry::request_intake(&dir, &root).unwrap();
        f.project("application");
        let connected = registry::register(&dir, &root).unwrap();
        registry::record(&dir, vec![stale], false).unwrap();
        let entries = registry::list(&dir).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].project_id, connected.project_id);
        assert_eq!(entries[0].state, ProjectState::Connected);
        assert_eq!(entries[0].last_used_at, connected.last_used_at);
    }
}
