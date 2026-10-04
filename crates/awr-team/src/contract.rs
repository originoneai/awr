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
        };
        contract.validate()?;
        Ok(contract)
    }
}

impl WorkContract {
    pub const CODEC: &'static str = "awr-team-contract-v1";
    pub const CODEC_V2: &'static str = "awr-team-contract-v2";

    pub fn validate(&self) -> TeamResult<()> {
        if !matches!(self.codec.as_str(), Self::CODEC | Self::CODEC_V2) {
            return Err(TeamError::InvalidContract(
                "unsupported contract codec".into(),
            ));
        }
        if (self.codec == Self::CODEC && !self.dependency_acceptance.is_empty())
            || (self.codec == Self::CODEC_V2
                && (self.dependency_acceptance.is_empty()
                    || self
                        .required_dependencies
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        != self.required_dependencies.len()))
            || self.dependency_acceptance.keys().any(|upstream| {
                upstream == self.work_id.as_str() || !self.required_dependencies.contains(upstream)
            })
        {
            return Err(TeamError::InvalidContract(
                "dependency_acceptance requires V2, a nonempty map and unique existing required predecessors".into(),
            ));
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
        if self.codec == Self::CODEC_V2 {
            value["dependency_acceptance"] = json!(self.dependency_acceptance);
        }
        let _ = canonical_json(&value)?;
        Ok(value)
    }
}
