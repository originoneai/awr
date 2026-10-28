use crate::{
    ProjectId, RequestId, ScopeId, TeamError, TeamResult, TenantId, WorkId, decode_u64,
    request_hash,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeSet;

pub(crate) fn text(value: &str, limit: usize, field: &str) -> TeamResult<()> {
    if value.trim().is_empty() || value.len() > limit || value.chars().any(char::is_control) {
        return Err(TeamError::InvalidInput(format!("invalid delivery {field}")));
    }
    Ok(())
}

pub(crate) fn digest(value: &str, field: &str) -> TeamResult<()> {
    if !awr_core::is_sha256_hash(value)
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(TeamError::InvalidInput(format!("invalid delivery {field}")));
    }
    Ok(())
}

pub(crate) fn names(values: &[String], field: &str) -> TeamResult<()> {
    if values.len() > 64 {
        return Err(TeamError::InvalidInput(format!(
            "too many delivery {field}"
        )));
    }
    let mut unique = BTreeSet::new();
    for value in values {
        text(value, 128, field)?;
        if !unique.insert(value) {
            return Err(TeamError::InvalidInput(format!(
                "duplicate delivery {field}"
            )));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisionFormat {
    GitSha1,
    GitSha256,
    Artifact,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevisionRef {
    /// Opaque resource identity, not a required GitHub URL.
    pub resource: String,
    pub format: RevisionFormat,
    pub value: String,
}

impl RevisionRef {
    pub fn validate(&self) -> TeamResult<()> {
        text(&self.resource, 1024, "revision resource")?;
        match self.format {
            RevisionFormat::GitSha1 | RevisionFormat::GitSha256 => {
                let length = if self.format == RevisionFormat::GitSha1 {
                    40
                } else {
                    64
                };
                if self.value.len() != length
                    || !self
                        .value
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return Err(TeamError::InvalidInput(
                        "invalid delivery Git revision".into(),
                    ));
                }
                Ok(())
            }
            RevisionFormat::Artifact => text(&self.value, 128, "artifact revision"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "revision",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum TargetPrecondition {
    Missing,
    Exact(RevisionRef),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryTarget {
    pub resource: String,
    pub reference: Option<String>,
    /// The adapter must actually verify absence or the expected revision.
    pub precondition: TargetPrecondition,
}

impl DeliveryTarget {
    pub fn validate(&self) -> TeamResult<()> {
        text(&self.resource, 1024, "target resource")?;
        if let Some(reference) = &self.reference {
            text(reference, 1024, "target reference")?;
        }
        if let TargetPrecondition::Exact(revision) = &self.precondition {
            revision.validate()?;
            if revision.resource != self.resource {
                return Err(TeamError::InvalidInput(
                    "target revision resource mismatch".into(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactEntry {
    pub artifact_id: String,
    pub sha256: String,
    /// Decimal string, including zero for an empty artifact.
    pub byte_length: String,
    /// Inspectable reference; validation does not open it.
    pub locator: String,
}

impl ArtifactEntry {
    pub fn validate(&self) -> TeamResult<()> {
        text(&self.artifact_id, 128, "artifact identity")?;
        digest(&self.sha256, "artifact digest")?;
        decode_u64(&self.byte_length)
            .map_err(|_| TeamError::InvalidInput("invalid delivery artifact byte length".into()))?;
        text(&self.locator, 4096, "artifact locator")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactManifest {
    pub entries: Vec<ArtifactEntry>,
}

impl ArtifactManifest {
    pub fn validate(&self) -> TeamResult<()> {
        if self.entries.is_empty() || self.entries.len() > 128 {
            return Err(TeamError::InvalidInput(
                "invalid delivery manifest size".into(),
            ));
        }
        let mut ids = BTreeSet::new();
        for entry in &self.entries {
            entry.validate()?;
            if !ids.insert(&entry.artifact_id) {
                return Err(TeamError::InvalidInput(
                    "duplicate delivery artifact identity".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn digest(&self) -> TeamResult<String> {
        self.validate()?;
        request_hash(&json!({"codec": "awr-delivery-manifest-v1", "manifest": self}))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateBinding {
    pub tenant_id: TenantId,
    pub project_id: ProjectId,
    pub scope_id: ScopeId,
    pub workstream_id: String,
    pub work_id: WorkId,
    pub candidate_id: RequestId,
    pub candidate_version: String,
    pub contract_hash: String,
    pub manifest_digest: String,
    pub source_revision: Option<RevisionRef>,
    /// Optional ordinary Git ref advertised by the source revision's remote.
    /// The ref is a fetch locator, not an immutable identity; consumers must
    /// verify the fetched commit against `source_revision`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_fetch_ref: Option<String>,
    pub required_checks: Vec<String>,
    pub target: DeliveryTarget,
}

/// Conservative provider-neutral ref syntax accepted at the submission boundary.
/// Git permits additional characters, but they are intentionally outside this
/// contract. Clients must still pass the value as one inert process argument
/// and must never construct a shell command from it.
pub const SOURCE_FETCH_REF_PATTERN: &str = r"^refs/(heads|tags)/[A-Za-z0-9._/+@-]+$";

/// Additional Git ref-shape exclusions applied after the portable character set.
/// These patterns are also emitted by delivery submission discovery.
pub const SOURCE_FETCH_REF_FORBIDDEN_PATTERNS: &[&str] =
    &[r"\.\.", r"(^|/)\.", r"(\.lock|\.)(/|$)", r"//|/$"];

fn valid_source_fetch_ref(value: &str) -> bool {
    (value.starts_with("refs/heads/") || value.starts_with("refs/tags/"))
        && text(value, 1024, "source fetch ref").is_ok()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._/+@-".contains(&byte))
        && !value.contains("..")
        && !value.contains("@{")
        && value.split('/').all(|part| {
            !part.is_empty()
                && !part.starts_with('.')
                && !part.ends_with('.')
                && !part.ends_with(".lock")
        })
}

impl CandidateBinding {
    pub fn validate(&self) -> TeamResult<()> {
        for id in [
            self.tenant_id.as_str(),
            self.project_id.as_str(),
            self.scope_id.as_str(),
            self.work_id.as_str(),
            self.candidate_id.as_str(),
        ] {
            text(id, 128, "binding identity")?;
        }
        text(&self.workstream_id, 128, "binding workstream")?;
        if decode_u64(&self.candidate_version)
            .map_err(|_| TeamError::InvalidInput("invalid delivery candidate version".into()))?
            == 0
        {
            return Err(TeamError::InvalidInput(
                "delivery candidate version must be positive".into(),
            ));
        }
        digest(&self.contract_hash, "contract hash")?;
        digest(&self.manifest_digest, "manifest digest")?;
        if let Some(revision) = &self.source_revision {
            revision.validate()?;
        }
        if let Some(reference) = &self.source_fetch_ref {
            if !valid_source_fetch_ref(reference)
                || !self.source_revision.as_ref().is_some_and(|revision| {
                    matches!(
                        revision.format,
                        RevisionFormat::GitSha1 | RevisionFormat::GitSha256
                    )
                })
            {
                return Err(TeamError::InvalidInput(
                    "invalid delivery source fetch ref".into(),
                ));
            }
        }
        names(&self.required_checks, "required checks")?;
        self.target.validate()
    }

    pub fn digest(&self) -> TeamResult<String> {
        self.validate()?;
        request_hash(&json!({"codec": "awr-delivery-binding-v1", "binding": self}))
    }
}

pub fn require_same_candidate(
    expected: &CandidateBinding,
    observed: &CandidateBinding,
) -> TeamResult<()> {
    expected.validate()?;
    observed.validate()?;
    if expected != observed {
        return Err(TeamError::InvalidInput(
            "delivery candidate binding changed; reevaluate".into(),
        ));
    }
    Ok(())
}
