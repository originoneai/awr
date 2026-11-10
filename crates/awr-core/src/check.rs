//! Check contracts and pure oracle hashes. Execution and persistence live in their own crates.
use crate::{Error, Id, Result, Revision};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const CHECK_EVENT: &str = "work.check";
pub const CHECK_PAYLOAD_CAP: usize = 16 * 1024;
pub const CHECK_FILE_CAP: u64 = 4 * 1024 * 1024;

// Oracle identity: file sizes and execution observations never change the declared oracle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OracleFile {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

pub fn example_oracle(id: &str, run: &[String], files: &[OracleFile]) -> Result<String> {
    let files: Vec<_> = files
        .iter()
        .map(|file| json!({"path": file.path, "sha256": file.sha256}))
        .collect();
    oracle_hash(
        "awr-example-oracle-v1\n",
        &json!({"id": id, "run": run, "files": files}),
    )
}

/// Callers supply only required examples with a check binding; order is by stable example ID.
pub fn oracle_fingerprint(oracles: &[(String, String)]) -> Result<String> {
    let mut sorted: Vec<_> = oracles.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    oracle_hash(
        "awr-oracle-v1\n",
        &json!(sorted.iter().map(|v| &v.1).collect::<Vec<_>>()),
    )
}

fn oracle_hash(domain: &str, value: &Value) -> Result<String> {
    let mut hash = Sha256::new();
    hash.update(domain.as_bytes());
    // serde_json's object map is ordered, matching the existing canonical JSON hashes.
    hash.update(serde_json::to_vec(value)?);
    Ok(format!("sha256:{:x}", hash.finalize()))
}

// Run observations. Later check gates and discrimination rules can build on these types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckPhase {
    Baseline,
    Acceptance,
    Mutation,
}
impl CheckPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Acceptance => "acceptance",
            Self::Mutation => "mutation",
        }
    }
    pub fn met(self, outcome: CheckOutcome) -> bool {
        outcome
            == if self == Self::Baseline {
                CheckOutcome::Failed
            } else {
                CheckOutcome::Passed
            }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckOutcome {
    Passed,
    Failed,
    Errored,
    Unknown,
}
impl CheckOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Errored => "errored",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckTreeState {
    Clean,
    Dirty,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckPayload {
    pub version: u32,
    pub example: String,
    pub criterion_sha256: String,
    pub phase: CheckPhase,
    pub outcome: CheckOutcome,
    pub reason: Option<String>,
    pub log_dir: String,
    pub exit_code: Option<i32>,
    pub example_oracle: Option<String>,
    pub oracle_files: Vec<OracleFile>,
    pub run_sha256: String,
    pub run_preview: String,
    pub source_sha: Option<String>,
    pub tree_state: CheckTreeState,
    pub stdout_sha256: String,
    pub stderr_sha256: String,
    pub started_at: i64,
    pub finished_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutants: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub survivors: Option<Vec<String>>,
}
impl CheckPayload {
    pub fn validate(&self) -> Result<()> {
        let hash = |s: &str| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit());
        let relative = |s: &str| {
            !s.is_empty()
                && !s.contains(['\\', ':'])
                && s.split('/').all(|p| !p.is_empty() && p != "." && p != "..")
        };
        let example_id = (1..=32).contains(&self.example.len())
            && self
                .example
                .starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
            && self
                .example
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if self.version != 1
            || !example_id
            || !hash(&self.criterion_sha256)
            || !hash(&self.run_sha256)
            || !hash(&self.stdout_sha256)
            || !hash(&self.stderr_sha256)
            || self.run_preview.len() > 256
            || self.started_at < 0
            || self.finished_at < self.started_at
            || !self.log_dir.starts_with(".awr/artifacts/checks/")
            || !relative(&self.log_dir)
            || self
                .source_sha
                .as_ref()
                .is_some_and(|s| !crate::is_source_sha(s))
            || self
                .example_oracle
                .as_ref()
                .is_some_and(|s| !s.strip_prefix("sha256:").is_some_and(hash))
            || self.oracle_files.iter().any(|f| {
                f.path.starts_with(['/', '\\'])
                    || f.path.split(['/', '\\']).any(|p| p == "..")
                    || f.path.trim().is_empty()
                    || !hash(&f.sha256)
                    || f.size > CHECK_FILE_CAP
            })
            || serde_json::to_vec(self)?.len() > CHECK_PAYLOAD_CAP
        {
            return Err(Error::InvalidInput(
                "invalid or oversized work.check observation".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct RecordCheckRequest {
    pub work: String,
    pub session: Id,
    pub expected_revision: Revision,
    pub payload: CheckPayload,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn check_oracle_hashes_the_versioned_canonical_document_without_file_sizes() {
        let mut files = vec![OracleFile {
            path: "tests/draft.rs".into(),
            sha256: "a".repeat(64),
            size: 42,
        }];
        let argv = vec!["draft-test".into(), "--exact".into()];
        let value = json!({"id":"draft", "run":argv, "files":[{"path":"tests/draft.rs","sha256":"a".repeat(64)}]});
        let expected = format!(
            "sha256:{:x}",
            Sha256::digest(format!("awr-example-oracle-v1\n{value}"))
        );
        assert_eq!(example_oracle("draft", &argv, &files).unwrap(), expected);
        files[0].size = 43;
        assert_eq!(example_oracle("draft", &argv, &files).unwrap(), expected);
        files[0].sha256 = "b".repeat(64);
        assert_ne!(example_oracle("draft", &argv, &files).unwrap(), expected);
        assert_ne!(example_oracle("other", &argv, &files).unwrap(), expected);
    }
    #[test]
    fn check_fingerprint_orders_by_id_and_keeps_individual_oracles_independent() {
        let a = vec![("z".into(), "hash-z".into()), ("a".into(), "hash-a".into())];
        let mut b = a.clone();
        b.reverse();
        assert_eq!(
            oracle_fingerprint(&a).unwrap(),
            oracle_fingerprint(&b).unwrap()
        );
        b[0].1 = "changed".into();
        assert_ne!(
            oracle_fingerprint(&a).unwrap(),
            oracle_fingerprint(&b).unwrap()
        );
    }
    #[test]
    fn check_timeout_and_errors_do_not_meet_baseline_or_acceptance() {
        assert!(CheckPhase::Baseline.met(CheckOutcome::Failed));
        assert!(CheckPhase::Acceptance.met(CheckOutcome::Passed));
        for phase in [
            CheckPhase::Baseline,
            CheckPhase::Acceptance,
            CheckPhase::Mutation,
        ] {
            assert!(!phase.met(CheckOutcome::Unknown));
            assert!(!phase.met(CheckOutcome::Errored));
        }
    }
}
