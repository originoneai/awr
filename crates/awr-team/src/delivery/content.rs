//! Complete snapshot observations are distinct from selected manifest entries.
use super::binding::text;
use super::{
    CandidateBinding, FactProvenance, FactSource, IntegrationObservation, IntegrationOutcome,
    RevisionFormat, RevisionRef, TargetPrecondition, require_same_candidate,
};
use crate::{RequestId, TeamError, TeamResult};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotIdentityFormat {
    GitTreeSha1,
    GitTreeSha256,
}

/// A complete repository tree, including paths, modes and all object identities.
/// A manifest digest or a caller-selected subset is not a snapshot identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompleteSnapshotIdentity {
    pub resource: String,
    pub format: SnapshotIdentityFormat,
    pub value: String,
}

impl CompleteSnapshotIdentity {
    fn revision_format(&self) -> RevisionFormat {
        match self.format {
            SnapshotIdentityFormat::GitTreeSha1 => RevisionFormat::GitSha1,
            SnapshotIdentityFormat::GitTreeSha256 => RevisionFormat::GitSha256,
        }
    }

    pub fn validate(&self) -> TeamResult<()> {
        RevisionRef {
            resource: self.resource.clone(),
            format: self.revision_format(),
            value: self.value.clone(),
        }
        .validate()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentProofUnavailableReason {
    NotObserved,
    HistoryUnavailable,
    ContentChanged,
    BaseChanged,
    TargetUnstable,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum IntegrationContentWitness {
    ExactRevision,
    MatchingCompleteSnapshots {
        source_snapshot: CompleteSnapshotIdentity,
        result_snapshot: CompleteSnapshotIdentity,
        /// The declared exact target must actually remain in both histories.
        retained_base: RevisionRef,
    },
    Unavailable {
        reason: ContentProofUnavailableReason,
    },
}

impl IntegrationContentWitness {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::ExactRevision => "exact_revision",
            Self::MatchingCompleteSnapshots { .. } => "matching_complete_snapshots",
            Self::Unavailable { .. } => "unavailable",
        }
    }
}

/// Description of an adapter's observed proof. Validation grants no authority,
/// performs no repository query and cannot establish human approval.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationContentProof {
    pub binding: CandidateBinding,
    pub request_id: Option<RequestId>,
    pub observation_reference: String,
    pub result_revision: Option<RevisionRef>,
    pub witness: IntegrationContentWitness,
    pub provenance: FactProvenance,
}

fn invalid() -> TeamError {
    TeamError::InvalidInput("invalid complete integration content proof".into())
}

impl IntegrationContentProof {
    pub fn validate(&self) -> TeamResult<()> {
        self.binding.validate()?;
        text(
            &self.observation_reference,
            4096,
            "content observation reference",
        )?;
        if let Some(request) = &self.request_id {
            text(request.as_str(), 128, "content proof request")?;
        }
        self.provenance.validate()?;
        if let Some(result) = &self.result_revision {
            result.validate()?;
            if result.resource != self.binding.target.resource {
                return Err(invalid());
            }
        }
        if matches!(self.witness, IntegrationContentWitness::Unavailable { .. }) {
            return Ok(());
        }
        let source = self.binding.source_revision.as_ref().ok_or_else(invalid)?;
        let result = self.result_revision.as_ref().ok_or_else(invalid)?;
        if !matches!(
            source.format,
            RevisionFormat::GitSha1 | RevisionFormat::GitSha256
        ) || source.resource != self.binding.target.resource
            || result.format != source.format
        {
            return Err(invalid());
        }
        match &self.witness {
            IntegrationContentWitness::ExactRevision if source == result => Ok(()),
            IntegrationContentWitness::MatchingCompleteSnapshots {
                source_snapshot,
                result_snapshot,
                retained_base,
            } => {
                source_snapshot.validate()?;
                result_snapshot.validate()?;
                retained_base.validate()?;
                if source_snapshot != result_snapshot
                    || source_snapshot.resource != source.resource
                    || source_snapshot.revision_format() != source.format
                    || retained_base.format != source.format
                    || self.binding.target.precondition
                        != TargetPrecondition::Exact(retained_base.clone())
                {
                    return Err(invalid());
                }
                Ok(())
            }
            _ => Err(invalid()),
        }
    }

    /// Check associations only. The ingesting store must independently prove
    /// connector authority, original version, same inbox and inspection timing.
    pub fn proves_observation(&self, observation: &IntegrationObservation) -> TeamResult<bool> {
        self.validate()?;
        require_same_candidate(&self.binding, &observation.binding)?;
        if self.request_id != observation.request_id
            || self.observation_reference != observation.external_reference
            || self.result_revision != observation.result_revision
            || self.provenance.source != FactSource::AdapterObservation
            || observation.provenance.source != FactSource::AdapterObservation
            || self.provenance.reference != observation.provenance.reference
            || self.provenance.observed_at_unix_ms != observation.provenance.observed_at_unix_ms
            || self.provenance.recorded_at_unix_ms != observation.provenance.recorded_at_unix_ms
        {
            return Err(invalid());
        }
        Ok(observation.outcome == IntegrationOutcome::Applied
            && !matches!(self.witness, IntegrationContentWitness::Unavailable { .. }))
    }
}
