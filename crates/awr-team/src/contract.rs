use crate::canonical::{canonical_json, contract_hash};
use crate::error::{TeamError, TeamResult};
use crate::ids::WorkId;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// A consumer explicitly selects this assurance basis for one required input.
/// Omission preserves the legacy policy; this never claims human acceptance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyAcceptanceMode {
    AgentReviewedCallerAssertedReconciled,
    SimulatedMemberIndependent,
    /// V6 declaration only. A runtime must verify an exact adopted export;
    /// an upstream completion receipt by itself never satisfies this mode.
    CrossWorkstream(CrossWorkstreamDependencyPolicy),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossWorkstreamReviewAssurance {
    TeamIndependent,
    SimulatedMemberIndependent,
}

/// Review identity and version selection are separate from execution assurance
/// and from the consumer's own completion policy. No field grants authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossWorkstreamDependencyPolicy {
    pub review_assurance: CrossWorkstreamReviewAssurance,
    pub version_policy: awr_core::DeliveryVersionPolicy,
}

/// An explicit agreement about workspace-local caller-managed effects. This
/// does not attest to process termination, OS isolation or artifact quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionSettlementMode {
    IndependentWorkspaceV1,
    /// Allows only the original current claim's terminal report after expiry.
    /// Admission, renewal and review retain their existing authority checks.
    IndependentWorkspaceV2,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionSettlementPolicy {
    pub mode: ExecutionSettlementMode,
    /// Stable opaque workspace identity; never a path or an authority grant.
    pub workspace_id: String,
}

impl ExecutionSettlementPolicy {
    pub const COMPLETION_POLICY: &'static str = "caller_managed_execution_and_agent_review";
    /// Explicit team simulation policy. The physical operator count is not an
    /// identity or approval grant; authenticated member independence is separate.
    pub const SIMULATED_MEMBER_COMPLETION_POLICY: &'static str =
        "caller_managed_execution_and_simulated_member_review";

    pub fn validate(&self) -> TeamResult<()> {
        let id = self.workspace_id.as_bytes();
        if id.is_empty()
            || id.len() > 128
            || !id[0].is_ascii_alphanumeric()
            || !id
                .iter()
                .all(|c| c.is_ascii_alphanumeric() || b"_.:-".contains(c))
        {
            return Err(TeamError::InvalidContract(
                "workspace_id must be a stable opaque identity of 1..128 ASCII bytes".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkDefinitionState {
    Draft,
    Enabled,
    Archived,
}

/// Semantic work definition used for Team contract hashing.
/// Progress notes and next_action are intentionally excluded.
/// The V1 contract is closed: unknown fields are rejected instead of being
/// silently dropped before hashing (CR #34 P2-2). Extension requires an
/// explicit codec/version bump.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "WireContract")]
pub struct WorkContract {
    pub codec: String,
    pub work_id: WorkId,
    pub external_key: String,
    pub goals: Vec<String>,
    pub hard_rules: Vec<String>,
    pub scope_paths: Vec<String>,
    pub acceptance: Vec<String>,
    pub required_dependencies: Vec<String>,
    pub completion_policy: String,
    pub verification_requirements: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub dependency_acceptance: BTreeMap<String, DependencyAcceptanceMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_settlement: Option<ExecutionSettlementPolicy>,
}

// V1 remains a closed wire contract, including rejection of an empty V2 field.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireContract {
    codec: String,
    work_id: WorkId,
    external_key: String,
    goals: Vec<String>,
    hard_rules: Vec<String>,
    scope_paths: Vec<String>,
    acceptance: Vec<String>,
    required_dependencies: Vec<String>,
    completion_policy: String,
    verification_requirements: Vec<String>,
    #[serde(default, deserialize_with = "present_modes")]
    dependency_acceptance: Option<BTreeMap<String, DependencyAcceptanceMode>>,
    #[serde(default, deserialize_with = "present_settlement")]
    execution_settlement: Option<ExecutionSettlementPolicy>,
}

fn present_settlement<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<ExecutionSettlementPolicy>, D::Error> {
    // A present null is invalid, not an omitted opt-in. Old codecs must never
    // silently ignore a requested settlement policy.
    ExecutionSettlementPolicy::deserialize(d).map(Some)
}

pub(crate) fn present_modes<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<BTreeMap<String, DependencyAcceptanceMode>>, D::Error> {
    struct Modes;
    impl<'de> serde::de::Visitor<'de> for Modes {
        type Value = BTreeMap<String, DependencyAcceptanceMode>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a dependency acceptance map with unique keys")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut input: M,
        ) -> Result<Self::Value, M::Error> {
            let mut modes = BTreeMap::new();
            while let Some((key, value)) = input.next_entry::<String, DependencyAcceptanceMode>()? {
                if modes.insert(key, value).is_some() {
                    return Err(serde::de::Error::custom(
                        "duplicate dependency acceptance key",
                    ));
                }
            }
            Ok(modes)
        }
    }
    d.deserialize_map(Modes).map(Some)
}

impl TryFrom<WireContract> for WorkContract {
    type Error = TeamError;

    fn try_from(wire: WireContract) -> TeamResult<Self> {
        if wire.codec == Self::CODEC && wire.dependency_acceptance.is_some() {
            return Err(TeamError::InvalidContract(
                "dependency_acceptance requires contract V2".into(),
            ));
        }
        if !matches!(
            wire.codec.as_str(),
            Self::CODEC_V3 | Self::CODEC_V4 | Self::CODEC_V5 | Self::CODEC_V6
        ) && wire.execution_settlement.is_some()
        {
            return Err(TeamError::InvalidContract(
                "execution_settlement requires contract V3/V4/V5/V6".into(),
            ));
        }
        let dependency_acceptance = wire.dependency_acceptance.unwrap_or_default();
        let contract = Self {
            codec: wire.codec,
            work_id: wire.work_id,
            external_key: wire.external_key,
            goals: wire.goals,
            hard_rules: wire.hard_rules,
            scope_paths: wire.scope_paths,
            acceptance: wire.acceptance,
            required_dependencies: wire.required_dependencies,
            completion_policy: wire.completion_policy,
            verification_requirements: wire.verification_requirements,
            dependency_acceptance,
            execution_settlement: wire.execution_settlement,
        };
        contract.validate()?;
        Ok(contract)
    }
}

impl WorkContract {
    pub const CODEC: &'static str = "awr-team-contract-v1";
    pub const CODEC_V2: &'static str = "awr-team-contract-v2";
    pub const CODEC_V3: &'static str = "awr-team-contract-v3";
    pub const CODEC_V4: &'static str = "awr-team-contract-v4";
    pub const CODEC_V5: &'static str = "awr-team-contract-v5";
    pub const CODEC_V6: &'static str = "awr-team-contract-v6";

    pub fn validate(&self) -> TeamResult<()> {
        if !matches!(
            self.codec.as_str(),
            Self::CODEC
                | Self::CODEC_V2
                | Self::CODEC_V3
                | Self::CODEC_V4
                | Self::CODEC_V5
                | Self::CODEC_V6
        ) {
            return Err(TeamError::InvalidContract(
                "unsupported contract codec".into(),
            ));
        }
        if (self.codec == Self::CODEC && !self.dependency_acceptance.is_empty())
            || (self.codec == Self::CODEC_V2 && self.dependency_acceptance.is_empty())
            || (matches!(
                self.codec.as_str(),
                Self::CODEC_V2 | Self::CODEC_V3 | Self::CODEC_V4 | Self::CODEC_V5 | Self::CODEC_V6
            ) && self
                .required_dependencies
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != self.required_dependencies.len())
            || self.dependency_acceptance.keys().any(|upstream| {
                upstream == self.work_id.as_str() || !self.required_dependencies.contains(upstream)
            })
        {
            return Err(TeamError::InvalidContract(
                "dependency_acceptance requires V2/V3/V4/V5/V6 and unique existing required predecessors; V2 requires a nonempty map".into(),
            ));
        }
        let simulated_dependency = self
            .dependency_acceptance
            .values()
            .any(|mode| *mode == DependencyAcceptanceMode::SimulatedMemberIndependent);
        let cross_stream_dependency = self
            .dependency_acceptance
            .values()
            .any(|mode| matches!(mode, DependencyAcceptanceMode::CrossWorkstream(_)));
        if cross_stream_dependency != (self.codec == Self::CODEC_V6) {
            return Err(TeamError::InvalidContract(
                "cross_workstream dependencies require V6; V6 requires at least one explicit cross-stream predecessor".into(),
            ));
        }
        if self.codec != Self::CODEC_V6 && simulated_dependency != (self.codec == Self::CODEC_V5) {
            return Err(TeamError::InvalidContract(
                "simulated_member_independent dependencies require V5; V5 requires at least one explicit simulated predecessor".into(),
            ));
        }
        if self.completion_policy == ExecutionSettlementPolicy::SIMULATED_MEMBER_COMPLETION_POLICY
            && !matches!(
                self.codec.as_str(),
                Self::CODEC_V4 | Self::CODEC_V5 | Self::CODEC_V6
            )
        {
            return Err(TeamError::InvalidContract(
                "simulated member review requires contract V4/V5/V6".into(),
            ));
        }
        match (&self.execution_settlement, self.codec.as_str()) {
            (Some(policy), Self::CODEC_V3 | Self::CODEC_V4 | Self::CODEC_V5 | Self::CODEC_V6) => {
                policy.validate()?;
                let supported_policy = match self.codec.as_str() {
                    Self::CODEC_V4 => {
                        self.completion_policy
                            == ExecutionSettlementPolicy::SIMULATED_MEMBER_COMPLETION_POLICY
                    }
                    Self::CODEC_V5 | Self::CODEC_V6 => matches!(
                        self.completion_policy.as_str(),
                        ExecutionSettlementPolicy::COMPLETION_POLICY
                            | ExecutionSettlementPolicy::SIMULATED_MEMBER_COMPLETION_POLICY
                    ),
                    _ => self.completion_policy == ExecutionSettlementPolicy::COMPLETION_POLICY,
                };
                if !supported_policy
                    || self.scope_paths.is_empty()
                    || self.scope_paths.iter().any(|p| p.trim().is_empty())
                    || self.verification_requirements.is_empty()
                    || self
                        .verification_requirements
                        .iter()
                        .any(|v| v.trim().is_empty())
                {
                    return Err(TeamError::InvalidContract(
                        "independent workspace settlement requires scoped paths, verification requirements and the codec's explicit review policy".into(),
                    ));
                }
            }
            (None, Self::CODEC | Self::CODEC_V2) => {}
            (None, Self::CODEC_V5 | Self::CODEC_V6)
                if self.completion_policy
                    != ExecutionSettlementPolicy::SIMULATED_MEMBER_COMPLETION_POLICY => {}
            _ => {
                return Err(TeamError::InvalidContract(
                    "execution_settlement is required for V3/V4 and simulated completion, forbidden in V1/V2, and otherwise optional in V5/V6".into(),
                ));
            }
        }
        if self.external_key.trim().is_empty() {
            return Err(TeamError::InvalidContract("external_key required".into()));
        }
        if self.acceptance.is_empty() {
            return Err(TeamError::InvalidContract("acceptance required".into()));
        }
        if self.completion_policy.trim().is_empty() {
            return Err(TeamError::InvalidContract(
                "completion_policy required".into(),
            ));
        }
        Ok(())
    }

    pub fn hash(&self) -> TeamResult<String> {
        self.validate()?;
        contract_hash(&self.canonical_fields()?)
    }

    fn canonical_fields(&self) -> TeamResult<Value> {
        let mut goals = self.goals.clone();
        let mut hard_rules = self.hard_rules.clone();
        let mut scope_paths = self.scope_paths.clone();
        let mut acceptance = self.acceptance.clone();
        let mut required_dependencies = self.required_dependencies.clone();
        let mut verification_requirements = self.verification_requirements.clone();
        goals.sort();
        hard_rules.sort();
        scope_paths.sort();
        acceptance.sort();
        required_dependencies.sort();
        verification_requirements.sort();
        let mut value = json!({
            "codec": self.codec,
            "work_id": self.work_id.as_str(),
            "external_key": self.external_key,
            "goals": goals,
            "hard_rules": hard_rules,
            "scope_paths": scope_paths,
            "acceptance": acceptance,
            "required_dependencies": required_dependencies,
            "completion_policy": self.completion_policy,
            "verification_requirements": verification_requirements,
        });
        if matches!(
            self.codec.as_str(),
            Self::CODEC_V2 | Self::CODEC_V3 | Self::CODEC_V4 | Self::CODEC_V5 | Self::CODEC_V6
        ) {
            value["dependency_acceptance"] = json!(self.dependency_acceptance);
        }
        if matches!(
            self.codec.as_str(),
            Self::CODEC_V3 | Self::CODEC_V4 | Self::CODEC_V5 | Self::CODEC_V6
        ) {
            value["execution_settlement"] = json!(self.execution_settlement);
        }
        let _ = canonical_json(&value)?;
        Ok(value)
    }
}
