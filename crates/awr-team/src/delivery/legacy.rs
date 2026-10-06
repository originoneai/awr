//! Read compatibility for incomplete historical PR observations.
use super::*;
use crate::{TeamError, TeamResult};
use serde::{Deserialize, Serialize};

/// All historical fields are retained verbatim. Names and flags confer no trust.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyPrSnapshot {
    pub delivery_id: String,
    pub repository: String,
    pub pr_number: i32,
    pub pr_url: String,
    pub head_sha: String,
    pub merge_sha: Option<String>,
    pub submitted: bool,
    pub approved: bool,
    pub merged: bool,
    pub fact_source: String,
    pub observed_at: String,
    pub contract_hash: String,
    pub state: String,
    pub test_evidence_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingDeliveryFact {
    CandidateIdentity,
    CandidateVersion,
    FullScopeBinding,
    Manifest,
    TargetPrecondition,
    RequiredChecks,
    VersionBoundVerification,
    AwrReviewDecision,
    IntegrationContentProof,
    SourceRevision,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyDeliveryIssue {
    InvalidSourceRevision,
    InvalidMergeRevision,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyExternalStatus {
    pub submitted: bool,
    pub approved: bool,
    pub merged: bool,
}

/// Deliberately not a DeliveryRecord: legacy facts cannot establish acceptance.
/// Raw PR fields remain on the old read surface instead of being duplicated here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyDeliveryObservation {
    pub protocol: String,
    pub fact_source: String,
    /// Original string, including its offset. No receipt-time substitution.
    pub observed_at: String,
    pub reported_source_revision: Option<RevisionRef>,
    pub reported_merge_revision: Option<RevisionRef>,
    pub external_status: LegacyExternalStatus,
    pub missing_facts: Vec<MissingDeliveryFact>,
    pub issues: Vec<LegacyDeliveryIssue>,
    pub acceptance_ready: bool,
}

impl LegacyPrSnapshot {
    /// Historical operations only supported SHA-1. Bad/unsupported data remains
    /// inspectable on the old surface with value-safe diagnostics here.
    fn revision(&self, value: &str) -> Option<RevisionRef> {
        let revision = RevisionRef {
            resource: self.repository.clone(),
            format: RevisionFormat::GitSha1,
            value: value.into(),
        };
        revision.validate().ok().map(|()| revision)
    }

    pub fn incomplete_observation(&self) -> LegacyDeliveryObservation {
        use MissingDeliveryFact::*;
        let mut missing_facts = vec![
            CandidateIdentity,
            CandidateVersion,
            FullScopeBinding,
            Manifest,
            TargetPrecondition,
            RequiredChecks,
            VersionBoundVerification,
            AwrReviewDecision,
            IntegrationContentProof,
        ];
        let mut issues = Vec::new();
        let reported_source_revision = self.revision(&self.head_sha);
        if reported_source_revision.is_none() {
            missing_facts.push(SourceRevision);
            issues.push(LegacyDeliveryIssue::InvalidSourceRevision);
        }
        let reported_merge_revision = self.merge_sha.as_deref().and_then(|value| {
            let revision = self.revision(value);
            if revision.is_none() {
                issues.push(LegacyDeliveryIssue::InvalidMergeRevision);
            }
            revision
        });
        LegacyDeliveryObservation {
            protocol: "awr-legacy-delivery-observation-v1".into(),
            fact_source: self.fact_source.clone(),
            observed_at: self.observed_at.clone(),
            reported_source_revision,
            reported_merge_revision,
            external_status: LegacyExternalStatus {
                submitted: self.submitted,
                approved: self.approved,
                merged: self.merged,
            },
            missing_facts,
            issues,
            acceptance_ready: false,
        }
    }

    /// A fully supplied, current candidate may associate the old provider locator.
    /// Scope/manifest/provenance are explicit inputs, never inferred from flags.
    /// This creates no verification, AWR decision or integration proof.
    pub fn change_request(
        &self,
        candidate: &DeliveryCandidate,
        current: &CandidateBinding,
        provenance: FactProvenance,
    ) -> TeamResult<ChangeRequest> {
        DeliveryRecord::Candidate(candidate.clone()).validate_against(current)?;
        let source = self.revision(&self.head_sha);
        if self.state != "active"
            || self.pr_number <= 0
            || source.is_none()
            || source.as_ref() != candidate.binding.source_revision.as_ref()
            || self.contract_hash != candidate.binding.contract_hash
        {
            return Err(TeamError::InvalidInput(
                "legacy delivery does not match current candidate".into(),
            ));
        }
        let record = ChangeRequest {
            binding: candidate.binding.clone(),
            provider: "github".into(),
            resource_id: format!("{}#{}", self.repository, self.pr_number),
            locator: Some(self.pr_url.clone()),
            provenance,
        };
        DeliveryEnvelope {
            protocol: DELIVERY_PROTOCOL.into(),
            protocol_version: DELIVERY_PROTOCOL_VERSION,
            record: DeliveryRecord::ChangeRequest(record.clone()),
        }
        .validate()?;
        Ok(record)
    }
}
