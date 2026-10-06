use super::binding::{digest, names, text};
use super::{
    ArtifactEntry, ArtifactManifest, CandidateBinding, RevisionFormat, RevisionRef,
    require_same_candidate,
};
use crate::{ActorId, RequestId, TeamError, TeamResult};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactSource {
    CallerDeclared,
    OperatorRecorded,
    AdapterObservation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactProvenance {
    pub source: FactSource,
    pub reference: String,
    /// None when no external observation time is known; never substitute receipt time.
    pub observed_at_unix_ms: Option<u64>,
    /// Assigned/confirmed by the ingesting store; caller text is not proof.
    pub recorded_at_unix_ms: u64,
}

impl FactProvenance {
    fn validate(&self) -> TeamResult<()> {
        text(&self.reference, 4096, "fact reference")?;
        for time in self
            .observed_at_unix_ms
            .into_iter()
            .chain([self.recorded_at_unix_ms])
        {
            if time == 0 || time > 9_007_199_254_740_991 {
                return Err(TeamError::InvalidInput(
                    "invalid delivery observation time".into(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryCandidate {
    pub binding: CandidateBinding,
    pub manifest: ArtifactManifest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeRequest {
    pub binding: CandidateBinding,
    pub provider: String,
    pub resource_id: String,
    pub locator: Option<String>,
    pub provenance: FactProvenance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationOutcome {
    Running,
    Passed,
    Failed,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationRun {
    pub binding: CandidateBinding,
    pub run_id: String,
    pub check: String,
    pub outcome: VerificationOutcome,
    pub result_artifact: Option<ArtifactEntry>,
    pub provenance: FactProvenance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewOutcome {
    Approved,
    Returned,
}

/// Reference to an existing AWR decision. External approvals belong in
/// external observations, never in this reference or the approval engine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewDecision {
    pub binding: CandidateBinding,
    pub round_id: String,
    pub decision_id: String,
    pub evidence_bundle_digest: String,
    pub reviewer_actor_id: ActorId,
    pub outcome: ReviewOutcome,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationOperation {
    FastForward,
    Merge,
    ApplyArtifacts,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationRef {
    pub check: String,
    pub run_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationRequest {
    pub binding: CandidateBinding,
    pub request_id: RequestId,
    pub operation: IntegrationOperation,
    pub review_round_id: String,
    pub review_decision_id: String,
    pub verified_checks: Vec<VerificationRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationOutcome {
    Pending,
    Applied,
    Rejected,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationObservation {
    pub binding: CandidateBinding,
    pub request_id: Option<RequestId>,
    pub external_reference: String,
    pub outcome: IntegrationOutcome,
    pub result_revision: Option<RevisionRef>,
    pub contains_manifest_digest: Option<String>,
    pub provenance: FactProvenance,
}

/// Describes mechanical support, never an authenticated action grant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterCapabilities {
    pub adapter_id: String,
    pub inspection: bool,
    pub change_requests: bool,
    pub verification: bool,
    pub integration_requests: bool,
    pub integration_observations: bool,
    pub notifications: bool,
    pub polling: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum DeliveryRecord {
    Candidate(DeliveryCandidate),
    ChangeRequest(ChangeRequest),
    Verification(VerificationRun),
    ReviewDecision(ReviewDecision),
    IntegrationRequest(IntegrationRequest),
    IntegrationObservation(IntegrationObservation),
    AdapterCapabilities(AdapterCapabilities),
}

impl DeliveryRecord {
    pub fn binding(&self) -> Option<&CandidateBinding> {
        Some(match self {
            Self::Candidate(record) => &record.binding,
            Self::ChangeRequest(record) => &record.binding,
            Self::Verification(record) => &record.binding,
            Self::ReviewDecision(record) => &record.binding,
            Self::IntegrationRequest(record) => &record.binding,
            Self::IntegrationObservation(record) => &record.binding,
            Self::AdapterCapabilities(_) => return None,
        })
    }

    /// Matching a current binding is necessary, not sufficient for acceptance.
    pub fn validate_against(&self, current: &CandidateBinding) -> TeamResult<()> {
        self.validate()?;
        require_same_candidate(current, self.binding().ok_or(TeamError::Unsupported)?)
    }

    pub fn validate(&self) -> TeamResult<()> {
        if let Some(binding) = self.binding() {
            binding.validate()?;
        }
        match self {
            Self::Candidate(record) => {
                if record.manifest.digest()? != record.binding.manifest_digest {
                    return Err(TeamError::InvalidInput(
                        "delivery manifest binding mismatch".into(),
                    ));
                }
            }
            Self::ChangeRequest(record) => {
                text(&record.provider, 128, "provider")?;
                text(&record.resource_id, 1024, "change request resource")?;
                if let Some(locator) = &record.locator {
                    text(locator, 4096, "change request locator")?;
                }
                record.provenance.validate()?;
            }
            Self::Verification(record) => {
                text(&record.run_id, 128, "verification run")?;
                text(&record.check, 128, "verification check")?;
                if let Some(artifact) = &record.result_artifact {
                    artifact.validate()?;
                }
                if record.outcome == VerificationOutcome::Passed && record.result_artifact.is_none()
                {
                    return Err(TeamError::InvalidInput(
                        "passed verification needs an inspectable result".into(),
                    ));
                }
                record.provenance.validate()?;
            }
            Self::ReviewDecision(record) => {
                text(&record.round_id, 128, "review round")?;
                text(&record.decision_id, 128, "review decision")?;
                text(record.reviewer_actor_id.as_str(), 128, "reviewer identity")?;
                digest(&record.evidence_bundle_digest, "evidence bundle")?;
            }
            Self::IntegrationRequest(record) => {
                text(record.request_id.as_str(), 128, "integration request")?;
                text(&record.review_round_id, 128, "integration review round")?;
                text(
                    &record.review_decision_id,
                    128,
                    "integration review decision",
                )?;
                let checks: Vec<_> = record
                    .verified_checks
                    .iter()
                    .map(|v| v.check.clone())
                    .collect();
                names(&checks, "verification references")?;
                if checks.iter().collect::<BTreeSet<_>>()
                    != record
                        .binding
                        .required_checks
                        .iter()
                        .collect::<BTreeSet<_>>()
                {
                    return Err(TeamError::InvalidInput(
                        "integration verification set mismatch".into(),
                    ));
                }
                for reference in &record.verified_checks {
                    text(&reference.run_id, 128, "verification reference")?;
                }
                if record.operation != IntegrationOperation::ApplyArtifacts
                    && !record
                        .binding
                        .source_revision
                        .as_ref()
                        .is_some_and(|revision| {
                            matches!(
                                revision.format,
                                RevisionFormat::GitSha1 | RevisionFormat::GitSha256
                            )
                        })
                {
                    return Err(TeamError::InvalidInput(
                        "Git integration needs a source Git revision".into(),
                    ));
                }
            }
            Self::IntegrationObservation(record) => {
                text(
                    &record.external_reference,
                    4096,
                    "integration observation reference",
                )?;
                if let Some(revision) = &record.result_revision {
                    revision.validate()?;
                    if revision.resource != record.binding.target.resource {
                        return Err(TeamError::InvalidInput(
                            "integration result target mismatch".into(),
                        ));
                    }
                }
                if let Some(value) = &record.contains_manifest_digest {
                    digest(value, "integrated manifest")?;
                    if value != &record.binding.manifest_digest {
                        return Err(TeamError::InvalidInput(
                            "integrated manifest mismatch".into(),
                        ));
                    }
                }
                if record.outcome == IntegrationOutcome::Applied
                    && (record.result_revision.is_none()
                        || record.contains_manifest_digest.is_none())
                {
                    return Err(TeamError::InvalidInput(
                        "applied integration needs a target revision and manifest observation"
                            .into(),
                    ));
                }
                record.provenance.validate()?;
            }
            Self::AdapterCapabilities(record) => {
                text(&record.adapter_id, 128, "adapter identity")?;
                if record.integration_requests
                    && !(record.inspection && record.integration_observations)
                {
                    return Err(TeamError::InvalidInput(
                        "integration adapter must support result inspection".into(),
                    ));
                }
            }
        }
        Ok(())
    }
}
