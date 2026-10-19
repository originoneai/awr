//! The authored-source / runtime-state boundary under `.awr/`.
use awr_source::{RESERVED_INTAKE_FILES, is_runtime_owned_path};
use std::{fs, path::PathBuf};

struct Project(PathBuf);
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn project(sources: &[(&str, &str)]) -> Project {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "awr-intake-boundary-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    fs::create_dir_all(root.join(".awr/intake")).unwrap();
    let mut manifest = String::from("[project]\nname = 'Boundary fixture'\n");
    for (path, adapter) in sources {
        manifest.push_str(&format!(
            "\n[[sources]]\ndomain = 'ledger'\nrole = '{}'\npath = '{path}'\nadapter = '{adapter}'\n",
            if manifest.contains("primary") { "supporting" } else { "primary" }
        ));
    }
    fs::write(root.join(".awr/project.toml"), manifest).unwrap();
    for name in [
        "work-ledger.yaml",
        "GOALS.md",
        "inventory.json",
        "project.toml",
    ] {
        fs::write(root.join(".awr/intake").join(name), "items: []\n").unwrap();
    }
    fs::write(root.join(".awr/state.db"), "").unwrap();
    fs::write(root.join("ledger.yaml"), "items: []\n").unwrap();
    Project(root.canonicalize().unwrap())
}

#[test]
fn ordinary_project_files_are_not_runtime_owned() {
    let p = project(&[("ledger.yaml", "yaml-ledger-v1")]);
    assert!(!is_runtime_owned_path(&p.0, &p.0.join("ledger.yaml")));
}

#[test]
fn registered_intake_source_is_authored() {
    let p = project(&[(".awr/intake/work-ledger.yaml", "yaml-ledger-v1")]);
    assert!(!is_runtime_owned_path(
        &p.0,
        &p.0.join(".awr/intake/work-ledger.yaml")
    ));
}

#[test]
fn unregistered_intake_file_is_runtime_owned() {
    let p = project(&[(".awr/intake/work-ledger.yaml", "yaml-ledger-v1")]);
    assert!(is_runtime_owned_path(
        &p.0,
        &p.0.join(".awr/intake/GOALS.md")
    ));
    assert!(is_runtime_owned_path(&p.0, &p.0.join(".awr/intake")));
}

#[test]
fn reserved_intake_files_stay_runtime_owned_even_if_registered() {
    let sources: Vec<(String, &str)> = RESERVED_INTAKE_FILES
        .iter()
        .map(|name| (format!(".awr/intake/{name}"), "yaml-ledger-v1"))
        .collect();
    let refs: Vec<(&str, &str)> = sources.iter().map(|(p, a)| (p.as_str(), *a)).collect();
    let p = project(&refs);
    for name in RESERVED_INTAKE_FILES {
        assert!(is_runtime_owned_path(
            &p.0,
            &p.0.join(".awr/intake").join(name)
        ));
    }
}

#[test]
fn other_runtime_state_is_runtime_owned() {
    let p = project(&[(".awr/state.db", "yaml-ledger-v1")]);
    assert!(is_runtime_owned_path(&p.0, &p.0.join(".awr/state.db")));
    assert!(is_runtime_owned_path(&p.0, &p.0.join(".awr/project.toml")));
}

#[test]
fn registered_directory_source_covers_its_files() {
    let p = project(&[(".awr/intake", "markdown-directory-v1")]);
    assert!(!is_runtime_owned_path(
        &p.0,
        &p.0.join(".awr/intake/GOALS.md")
    ));
    assert!(is_runtime_owned_path(
        &p.0,
        &p.0.join(".awr/intake/inventory.json")
    ));
}

#[cfg(unix)]
#[test]
fn symlinks_are_resolved_before_classification() {
    let p = project(&[
        ("ledger-link.yaml", "yaml-ledger-v1"),
        (".awr/intake/escape.yaml", "yaml-ledger-v1"),
    ]);
    // A project file that points into runtime state is runtime-owned.
    std::os::unix::fs::symlink(p.0.join(".awr/state.db"), p.0.join("ledger-link.yaml")).unwrap();
    assert!(is_runtime_owned_path(&p.0, &p.0.join("ledger-link.yaml")));
    // A registered intake name that points at a reserved file is runtime-owned.
    std::os::unix::fs::symlink(
        p.0.join(".awr/intake/inventory.json"),
        p.0.join(".awr/intake/escape.yaml"),
    )
    .unwrap();
    assert!(is_runtime_owned_path(
        &p.0,
        &p.0.join(".awr/intake/escape.yaml")
    ));
}

#[test]
fn unreadable_manifest_fails_closed() {
    let p = project(&[(".awr/intake/work-ledger.yaml", "yaml-ledger-v1")]);
    fs::write(p.0.join(".awr/project.toml"), "not toml [").unwrap();
    assert!(is_runtime_owned_path(
        &p.0,
        &p.0.join(".awr/intake/work-ledger.yaml")
    ));
}
