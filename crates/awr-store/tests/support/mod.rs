use awr_core::*;
use awr_store::{SourceRegistration, Store};
use std::path::{Path, PathBuf};

pub struct Fixture {
    pub root: PathBuf,
    pub store: Store,
    pub project: Project,
    pub source: Source,
}
impl Fixture {
    pub fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-query-{}", Id::new()));
        std::fs::create_dir(&root).unwrap();
        let mut store = Store::open(&root.join("state.db")).unwrap();
        let project = store.register_project(&root, "example", "Example").unwrap();
        let source = store
            .register_source(
                project.id,
                &SourceRegistration {
                    domain: "ledger",
                    role: "primary",
                    locator: "file:///example/ledger.yaml",
                    format: "yaml",
                    adapter: "yaml-ledger-v1",
                },
            )
            .unwrap();
        Self {
            root,
            store,
            project,
            source,
        }
    }
    /// Build a fixture under an explicit directory/locator (for real worktree
    /// reproductions): the store lives at `root/state.db` and the source points
    /// at the given locator.
    pub fn new_under(root: &Path, locator: &str) -> Self {
        std::fs::create_dir_all(root).unwrap();
        let mut store = Store::open(&root.join("state.db")).unwrap();
        let project = store.register_project(root, "example", "Example").unwrap();
        let source = store
            .register_source(
                project.id,
                &SourceRegistration {
                    domain: "ledger",
                    role: "primary",
                    locator: locator.into(),
                    format: "yaml",
                    adapter: "yaml-ledger-v1",
                },
            )
            .unwrap();
        Self {
            root: root.to_path_buf(),
            store,
            project,
            source,
        }
    }
    pub fn meta(&self, key: &str) -> ProjectionMeta {
        ProjectionMeta {
            id: Id::new(),
            external_key: key.into(),
            revision: 1,
            source_ref: SourceRef {
                source_id: self.source.id,
                locator: self.source.locator.clone(),
                source_revision: self.source.revision + 1,
                source_fingerprint: "snapshot-1".into(),
                pointer: Some(format!("/{key}")),
                start_line: None,
                end_line: None,
                section_fingerprint: None,
            },
        }
    }
    pub fn commit(&mut self, batch: ProjectionBatch) {
        self.source = self
            .store
            .commit_source_projection(&self.source, "snapshot-1", batch)
            .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
