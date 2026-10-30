//! Governance in a Team publish package: carried as one canonical file, bound by its hash, naming
//! only the work the ledger has, and never changing a contract.
use awr_core::{AdmissionPolicy, DefinitionSummary};
use awr_source::*;
use awr_team::{GOVERNANCE_FILE, GOVERNANCE_VERSION, GovernanceCandidate, WorkGovernance};
use serde_json::json;
use std::{fs, path::PathBuf};

const LEDGER: &str = r#"workstreams:
  version: 1
  definitions:
    - id: "00000000000000000000000001"
      external_key: app
      title: App
      state: active
      authority_version: 1
      goal_keys: [ship]
      acceptance_contracts: []
work_items:
  - id: a
    title: First
    status: planned
    workstream: app
    goals: [ship]
    acceptance: [verified]
  - id: b
    title: Second
    status: planned
    workstream: app
    goals: [ship]
    acceptance: [verified]
"#;

struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-source-gov-{}", awr_core::Id::new()));
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        fs::write(root.join("ledger.yaml"), LEDGER).unwrap();
        Self { root }
    }
    fn package(
        &self,
        governance: Option<GovernanceCandidate>,
    ) -> awr_core::Result<TeamPublishPackage> {
        let location = SoleSourceLocation::server_directory(&self.root, "ledger.yaml").unwrap();
        prepare_publish_from_ledger_bytes(
            &location,
            &self.root,
            LEDGER.as_bytes(),
            "fixture-project",
            &PublishPrepOptions {
                governance,
                ..PublishPrepOptions::default()
            },
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

fn candidate(keys: &[&str]) -> GovernanceCandidate {
    GovernanceCandidate {
        version: GOVERNANCE_VERSION,
        policy: serde_json::from_value::<AdmissionPolicy>(
            json!({"version": 1, "mode": "governed_enforce", "scope": "all"}),
        )
        .unwrap(),
        works: keys
            .iter()
            .map(|key| WorkGovernance {
                external_key: (*key).into(),
                tags: vec![],
                definition: None,
                summary: DefinitionSummary {
                    declared: false,
                    intent_adopted: false,
                    gaps: vec![],
                },
            })
            .collect(),
    }
}

#[test]
fn a_package_without_governance_carries_none() {
    let f = Fixture::new();
    let package = f.package(None).unwrap();
    assert!(package.governance_digest.is_none());
    assert!(
        package
            .files
            .iter()
            .all(|file| file.path != GOVERNANCE_FILE)
    );
    // A package written before governance existed still reads.
    let mut wire = serde_json::to_value(&package).unwrap();
    assert!(wire.get("governance_digest").is_none());
    wire.as_object_mut().unwrap().remove("governance_digest");
    assert_eq!(
        serde_json::from_value::<TeamPublishPackage>(wire).unwrap(),
        package
    );
}

#[test]
fn governance_travels_as_one_canonical_file_bound_by_its_hash() {
    let f = Fixture::new();
    let plain = f.package(None).unwrap();
    let governed = candidate(&["a", "b"]);
    let package = f.package(Some(governed.clone())).unwrap();
    assert_eq!(package.governance_digest, Some(governed.hash().unwrap()));
    let file = package
        .files
        .iter()
        .find(|file| file.path == GOVERNANCE_FILE)
        .expect("the governance file is part of the package");
    assert_eq!(
        serde_json::from_slice::<GovernanceCandidate>(&file.bytes).unwrap(),
        governed
    );
    // Files stay in path order and every other file is exactly as it was.
    let paths: Vec<&str> = package
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted);
    for file in &plain.files {
        assert!(package.files.contains(file), "{} changed", file.path);
    }
    assert_eq!(package.files.len(), plain.files.len() + 1);
    // Describing governance changes no contract, no graph and no identity.
    assert_eq!(package.bundle_digest, plain.bundle_digest);
    assert_eq!(package.graph_digest, plain.graph_digest);
    assert_eq!(package.ledger_identity_digest, plain.ledger_identity_digest);
    assert_eq!(package.source_version_digest, plain.source_version_digest);
    // The same governance always gives the same package.
    assert_eq!(package, f.package(Some(governed)).unwrap());
    // A different governance gives a different digest.
    let other = f.package(Some(candidate(&["a"]))).unwrap();
    assert_ne!(other.governance_digest, package.governance_digest);
}

#[test]
fn governance_cannot_name_work_the_ledger_does_not_have() {
    let f = Fixture::new();
    let error = f
        .package(Some(candidate(&["a", "zzz"])))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("zzz") && error.contains("does not have"),
        "{error}"
    );
}

#[test]
fn governance_that_is_not_valid_or_not_canonical_is_refused() {
    let f = Fixture::new();
    assert!(f.package(Some(candidate(&["b", "a"]))).is_err(), "unsorted");
    assert!(f.package(Some(candidate(&["a", "a"]))).is_err(), "repeated");
    let mut unknown = candidate(&["a"]);
    unknown.version = 9;
    assert!(f.package(Some(unknown)).is_err(), "unknown version");
}
