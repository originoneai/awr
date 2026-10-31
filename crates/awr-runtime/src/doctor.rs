use awr_core::{Error, Id, Result, Revision};
use awr_source::{Manifest, MarkdownDirectoryAdapter, source_adapter};
use awr_store::{DoctorReport, RuntimeFinding, Store};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Debug, Serialize)]
pub struct ProjectDoctorReport {
    /// Keep the original top-level SQLite fields; ok includes the additional checks.
    #[serde(flatten)]
    pub database: DoctorReport,
    pub database_ok: bool,
    pub read_only: bool,
    pub source_refresh_performed: bool,
    pub project_id: Option<Id>,
    pub project_revision: Option<Revision>,
    pub observed_final_revision: Option<Revision>,
    pub checked_at: i64,
    pub findings: Vec<RuntimeFinding>,
    pub sources_checked: usize,
    pub artifacts_checked: usize,
    pub limitations: Vec<String>,
}

fn finding(
    code: &str,
    severity: &str,
    kind: &str,
    id: impl ToString,
    message: impl ToString,
) -> RuntimeFinding {
    RuntimeFinding {
        code: code.into(),
        severity: severity.into(),
        object_kind: kind.into(),
        object_id: id.to_string(),
        message: message.to_string(),
        repair: None,
    }
}

/// Findings that advise and never fail `doctor`: a project that was healthy for the exit code stays so.
const ADVISORY_FINDINGS: &[&str] = &["claim_without_expiry"];

/// Inspect current files without calling the source indexer or changing retained freshness.
pub fn diagnose_project(
    root: &Path,
    database: &Path,
    max_bytes: u64,
) -> Result<ProjectDoctorReport> {
    if max_bytes == 0 || max_bytes > 1024 * 1024 * 1024 {
        return Err(Error::InvalidInput(
            "doctor artifact read cap must be 1..1073741824 bytes".into(),
        ));
    }
    let database_report = Store::inspect(database)?;
    let mut report = ProjectDoctorReport {
        database_ok: database_report.ok, database: database_report, read_only: true, source_refresh_performed: false,
        project_id: None, project_revision: None, observed_final_revision: None, checked_at: awr_core::now_millis()?,
        findings: vec![], sources_checked: 0, artifacts_checked: 0,
        limitations: vec![
            "Active status and age do not prove process liveness; explicit interruption requires selecting a session.".into(),
            "Pending operations and unregistered artifact files may belong to a live writer. Diagnosis does not complete, delete or repair them.".into(),
            "Source reads are capped at 16 MiB each. Registered artifact content is hashed within the requested cap and never emitted.".into(),
        ],
    };
    if report.database.integrity != ["ok"] {
        report.findings.push(finding("runtime_checks_unavailable", "error", "database", "integrity", "Runtime/file checks require readable database pages; no repair or migration was attempted."));
        return Ok(report);
    }
    let root = root.canonicalize()?;
    let store = match Store::open_readonly(database) {
        Ok(store) => store,
        Err(error) => {
            report.database.ok = false;
            report.findings.push(finding(
                "runtime_checks_unavailable",
                "error",
                "database",
                "schema",
                error,
            ));
            return Ok(report);
        }
    };
    let project = match store.project_by_root(&root) {
        Ok(project) => project,
        Err(error) => {
            report.database.ok = false;
            report.findings.push(finding(
                "project_unavailable",
                "error",
                "project",
                root.display(),
                error,
            ));
            return Ok(report);
        }
    };
    let inspected = store.inspect_runtime(project.id, report.checked_at)?;
    report.project_id = Some(project.id);
    report.project_revision = Some(inspected.project_revision);
    report.findings = inspected.findings;
    let manifest = match Manifest::load(&root) {
        Ok(manifest) => Some(manifest),
        Err(error) => {
            report.findings.push(finding(
                "source_manifest_unavailable",
                "error",
                "manifest",
                ".awr/project.toml",
                error,
            ));
            None
        }
    };
    if let Err(error) = awr_source::ClaimPolicy::load(&root) {
        report.findings.push(finding(
            "claim_policy_unreadable",
            "error",
            "policy",
            awr_source::POLICY_FILE,
            error,
        ));
    }
    if let Some(manifest) = &manifest {
        inspect_sources(&store, project.id, &root, manifest, &mut report)?;
    }
    inspect_artifacts(
        &store,
        project.id,
        &root,
        manifest.as_ref(),
        max_bytes,
        &mut report,
    )?;
    let final_revision = store.project(project.id)?.project_revision;
    report.observed_final_revision = Some(final_revision);
    if final_revision != inspected.project_revision {
        report.findings.push(finding(
            "concurrent_project_change",
            "warning",
            "project",
            project.id,
            "Project changed during inspection; rerun doctor for a consistent observation.",
        ));
    }
    report.findings.sort_by(|a, b| {
        (&a.code, &a.object_kind, &a.object_id).cmp(&(&b.code, &b.object_kind, &b.object_id))
    });
    if !report.database_ok {
        for finding in &mut report.findings {
            finding.repair = None;
        }
        report.limitations.push("Foreign-key problems remain visible through read-only runtime diagnostics. Explicit runtime repairs require database integrity first; no automatic reference reconstruction is offered.".into());
    }
    report.database.ok = report.database_ok
        && !report
            .findings
            .iter()
            .any(|f| f.severity != "info" && !ADVISORY_FINDINGS.contains(&f.code.as_str()));
    Ok(report)
}

fn inspect_sources(
    store: &Store,
    project: Id,
    root: &Path,
    manifest: &Manifest,
    report: &mut ProjectDoctorReport,
) -> Result<()> {
    let sources = store.sources(project)?;
    let indexed = sources
        .iter()
        .map(|s| ((s.domain.clone(), s.locator.clone()), s))
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    let mut failed_mappings = BTreeSet::new();
    for spec in &manifest.sources {
        let mapping = format!(
            "{}|{}",
            spec.domain,
            spec.locator.clone().unwrap_or_else(|| spec
                .path
                .as_ref()
                .unwrap()
                .to_string_lossy()
                .into_owned())
        );
        let adapter = source_adapter(&spec.adapter)?;
        let locators = match adapter.discover(root, manifest, spec) {
            Ok(locators) => locators,
            Err(error) => {
                failed_mappings.insert(mapping.clone());
                report.findings.push(finding(
                    "source_access_failed",
                    "error",
                    "source_mapping",
                    &mapping,
                    error,
                ));
                continue;
            }
        };
        for locator in locators {
            let identity = if spec.adapter == "markdown-directory-v1" {
                MarkdownDirectoryAdapter.source_identity(root, manifest, spec, &locator)
            } else {
                locator.identity()
            };
            let identity = match identity {
                Ok(identity) => identity,
                Err(error) => {
                    report.findings.push(finding(
                        "source_access_failed",
                        "error",
                        "source_mapping",
                        &mapping,
                        error,
                    ));
                    failed_mappings.insert(mapping.clone());
                    continue;
                }
            };
            let key = (spec.domain.clone(), identity.clone());
            if !seen.insert(key.clone()) {
                report.findings.push(finding(
                    "source_mapping_conflict",
                    "error",
                    "source",
                    &identity,
                    "Multiple manifest mappings select this source.",
                ));
                continue;
            }
            let source = indexed.get(&key).copied();
            if let Some(source) = source {
                let expected_config = awr_source::source_configuration(
                    spec,
                    manifest.project.context_profile == awr_source::ContextProfile::Minimal,
                );
                if source.config != expected_config
                    || source.role != spec.role
                    || source.adapter != spec.adapter
                {
                    report.findings.push(finding("source_configuration_changed", "warning", "source", source.id, "Current manifest configuration differs from the indexed source; reindex required."));
                }
            } else {
                report.findings.push(finding(
                    "source_not_indexed",
                    "warning",
                    "source",
                    &identity,
                    "Current manifest source has no active projection; reindex required.",
                ));
            }
            match locator.read(root, awr_source::source_read_cap(&spec.adapter)?) {
                Ok(snapshot) => {
                    report.sources_checked += 1;
                    if let Some(source) = source {
                        if snapshot.fingerprint != source.fingerprint {
                            report.findings.push(finding("source_fingerprint_changed", "warning", "source", source.id, format!("Observed {}; indexed {}. Retained projection was not changed; reindex required.",snapshot.fingerprint,source.fingerprint)));
                        }
                    }
                }
                Err(error) => report.findings.push(finding(
                    "source_access_failed",
                    "error",
                    "source",
                    source.map(|s| s.id.to_string()).unwrap_or(identity),
                    error,
                )),
            }
        }
    }
    for source in sources {
        let failed = source
            .config
            .get("mapping_key")
            .and_then(|v| v.as_str())
            .is_some_and(|key| failed_mappings.contains(key));
        if !seen.contains(&(source.domain.clone(), source.locator.clone())) && !failed {
            report.findings.push(finding("source_no_longer_selected", "warning", "source", source.id, "Source is retained as active but no longer selected by the manifest/directory inventory; reindex required."));
        }
    }
    Ok(())
}

fn inspect_artifacts(
    store: &Store,
    project: Id,
    root: &Path,
    manifest: Option<&Manifest>,
    cap: u64,
    report: &mut ProjectDoctorReport,
) -> Result<()> {
    let artifacts = store.artifacts(project)?;
    let mut allowed = vec![root.to_path_buf()];
    if let Some(manifest) = manifest {
        match manifest.authorized_roots(root) {
            Ok(roots) => allowed = roots,
            Err(error) => report.findings.push(finding(
                "authorized_roots_unavailable",
                "error",
                "manifest",
                ".awr/project.toml",
                error,
            )),
        }
    }
    let mut registered = BTreeSet::new();
    for artifact in artifacts {
        if artifact.locator.contains("://") {
            report.findings.push(finding(
                "artifact_unverified",
                "warning",
                "artifact",
                artifact.id,
                "Non-local artifact was not fetched; inspect its registered locator.",
            ));
            continue;
        }
        let path = if Path::new(&artifact.locator).is_absolute() {
            PathBuf::from(&artifact.locator)
        } else {
            root.join(&artifact.locator)
        };
        registered.insert(path.clone());
        let resolved = match path.canonicalize() {
            Ok(path) => path,
            Err(error) => {
                report.findings.push(finding(
                    "artifact_missing",
                    "error",
                    "artifact",
                    artifact.id,
                    error,
                ));
                continue;
            }
        };
        if !allowed.iter().any(|allowed| resolved.starts_with(allowed)) {
            report.findings.push(finding(
                "artifact_path_unavailable",
                "error",
                "artifact",
                artifact.id,
                "Artifact is outside the current authorized roots; content was not read.",
            ));
            continue;
        }
        registered.insert(resolved.clone());
        match hash_file(&resolved, artifact.size, cap) {
            Ok(digest) => {
                report.artifacts_checked += 1;
                if !digest.eq_ignore_ascii_case(&artifact.sha256) {
                    report.findings.push(finding("artifact_digest_mismatch", "error", "artifact", artifact.id, "File SHA256 differs from registered metadata; restore the original file or register a separate new artifact."));
                }
            }
            Err(error) => report.findings.push(finding(
                "artifact_unverified",
                "error",
                "artifact",
                artifact.id,
                error,
            )),
        }
    }
    let directory = root.join(".awr/artifacts");
    match fs::symlink_metadata(&directory) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            report.findings.push(finding(
                "artifact_directory_unavailable",
                "error",
                "artifact_directory",
                directory.display(),
                error,
            ));
            return Ok(());
        }
    }
    let scan = (|| -> Result<()> {
        let directory = directory.canonicalize()?;
        if !directory.starts_with(root) || !directory.is_dir() {
            return Err(Error::RuleViolation(
                "managed artifact directory escapes project or is not a directory".into(),
            ));
        }
        let mut entries = fs::read_dir(&directory)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            if !registered.contains(&entry.path()) {
                report.findings.push(finding("orphan_artifact", "warning", "artifact_file", entry.path().display(), "Managed-storage entry has no registered artifact. It may belong to an in-flight import; preserve it until its writer/history is checked. No file was deleted."));
            }
        }
        Ok(())
    })();
    if let Err(error) = scan {
        report.findings.push(finding(
            "artifact_directory_unavailable",
            "error",
            "artifact_directory",
            directory.display(),
            error,
        ));
    }
    Ok(())
}

fn hash_file(path: &Path, expected_size: u64, cap: u64) -> Result<String> {
    let mut file = File::open(path)?;
    let before = file.metadata()?;
    if !before.is_file() || before.len() != expected_size {
        return Err(Error::SourceConflict(
            "artifact file type/size differs from registered metadata".into(),
        ));
    }
    if before.len() > cap {
        return Err(Error::InvalidInput(format!(
            "artifact exceeds {cap} byte inspection cap; increase --max-bytes"
        )));
    }
    let mut digest = Sha256::new();
    let mut size = 0;
    let mut buffer = [0u8; 32 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        size += n as u64;
        if size > cap {
            return Err(Error::SourceConflict(
                "artifact grew beyond the inspection cap".into(),
            ));
        }
        digest.update(&buffer[..n]);
    }
    let after = file.metadata()?;
    if size != before.len()
        || after.len() != before.len()
        || after.modified()? != before.modified()?
    {
        return Err(Error::SourceConflict(
            "artifact changed during inspection".into(),
        ));
    }
    Ok(format!("{:x}", digest.finalize()))
}
