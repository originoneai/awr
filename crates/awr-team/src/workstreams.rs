//! Explicit multi-work source codec. Work branches keep their existing scope ID;
//! workstream ownership is a separate, source-backed dimension.
use crate::{DependencyAcceptanceMode, TeamError, TeamResult, WorkContract, contract_hash};
use awr_core::{Id, WorkstreamCatalog, WorkstreamWorkBinding, validate_workstream_ownership};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkstreamContract {
    pub workstream_id: Id,
    pub contract: WorkContract,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkstreamBundle {
    pub codec: String,
    pub catalog: WorkstreamCatalog,
    pub contracts: Vec<WorkstreamContract>,
}

impl WorkstreamBundle {
    pub const CODEC: &'static str = "awr-team-workstreams-v1";
    pub const CODEC_V2: &'static str = "awr-team-workstreams-v2";
    pub const CODEC_V3: &'static str = "awr-team-workstreams-v3";
    pub const CODEC_V4: &'static str = "awr-team-workstreams-v4";
    pub const CODEC_V5: &'static str = "awr-team-workstreams-v5";
    pub const CODEC_V6: &'static str = "awr-team-workstreams-v6";

    pub fn validate(&self, project_id: &str) -> TeamResult<()> {
        if !matches!(
            self.codec.as_str(),
            Self::CODEC
                | Self::CODEC_V2
                | Self::CODEC_V3
                | Self::CODEC_V4
                | Self::CODEC_V5
                | Self::CODEC_V6
        ) || self.catalog.project_id != project_id
        {
            return Err(TeamError::InvalidContract(
                "workstream codec or project mismatch".into(),
            ));
        }
        if self.contracts.is_empty() || self.contracts.len() > 10_000 {
            return Err(TeamError::InvalidContract(
                "workstream source needs 1..10000 contracts".into(),
            ));
        }
        let mut keys = BTreeSet::new();
        for entry in &self.contracts {
            entry.contract.validate()?;
            if self.codec == Self::CODEC && entry.contract.codec != WorkContract::CODEC {
                return Err(TeamError::InvalidContract(
                    "extended contracts require an explicit extended workstream codec".into(),
                ));
            }
            if (self.codec == Self::CODEC_V2
                && matches!(
                    entry.contract.codec.as_str(),
                    WorkContract::CODEC_V3
                        | WorkContract::CODEC_V4
                        | WorkContract::CODEC_V5
                        | WorkContract::CODEC_V6
                ))
                || (self.codec == Self::CODEC_V3
                    && matches!(
                        entry.contract.codec.as_str(),
                        WorkContract::CODEC_V4 | WorkContract::CODEC_V5 | WorkContract::CODEC_V6
                    ))
                || (self.codec == Self::CODEC_V4
                    && matches!(
                        entry.contract.codec.as_str(),
                        WorkContract::CODEC_V5 | WorkContract::CODEC_V6
                    ))
                || (self.codec == Self::CODEC_V5 && entry.contract.codec == WorkContract::CODEC_V6)
            {
                return Err(TeamError::InvalidContract(
                    "contract codec is newer than the workstream codec".into(),
                ));
            }
            for (upstream, mode) in &entry.contract.dependency_acceptance {
                let provider = self
                    .contracts
                    .iter()
                    .find(|provider| provider.contract.work_id.as_str() == upstream)
                    .ok_or_else(|| TeamError::InvalidContract("Agent dependency acceptance requires an existing predecessor in the same workstream; cross-stream adoption is unsupported".into()))?;
                if matches!(mode, DependencyAcceptanceMode::CrossWorkstream(_)) {
                    if self.codec != Self::CODEC_V6 || provider.workstream_id == entry.workstream_id
                    {
                        return Err(TeamError::InvalidContract(
                            "cross_workstream requires V6 and different source-owned workstreams"
                                .into(),
                        ));
                    }
                } else if provider.workstream_id != entry.workstream_id {
                    return Err(TeamError::InvalidContract("Agent dependency acceptance requires an existing predecessor in the same workstream; cross-stream adoption is unsupported".into()));
                }
            }
            if !keys.insert(&entry.contract.external_key) {
                return Err(TeamError::InvalidContract("duplicate work key".into()));
            }
        }
        let ids = self
            .contracts
            .iter()
            .map(|e| e.contract.work_id.as_str().to_owned())
            .collect::<Vec<_>>();
        let bindings = self
            .contracts
            .iter()
            .map(|e| WorkstreamWorkBinding {
                project_id: project_id.into(),
                work_item_id: e.contract.work_id.as_str().into(),
                workstream_id: e.workstream_id,
            })
            .collect::<Vec<_>>();
        validate_workstream_ownership(&self.catalog, &ids, &bindings)
            .map_err(|e| TeamError::InvalidContract(e.to_string()))?;
        if self.codec == Self::CODEC_V6 {
            self.validate_v6_dependencies()?;
        }
        Ok(())
    }

    fn validate_v6_dependencies(&self) -> TeamResult<()> {
        if !self
            .contracts
            .iter()
            .any(|e| e.contract.codec == WorkContract::CODEC_V6)
        {
            return Err(TeamError::InvalidContract(
                "V6 bundle requires an explicit V6 cross-stream contract".into(),
            ));
        }
        let by_id = self
            .contracts
            .iter()
            .map(|e| (e.contract.work_id.as_str(), e))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut edges = Vec::new();
        for entry in &self.contracts {
            let mut predecessors = std::collections::BTreeSet::new();
            for upstream in &entry.contract.required_dependencies {
                if !predecessors.insert(upstream) {
                    return Err(TeamError::InvalidContract(
                        "V6 required dependency graph must not contain duplicate edges".into(),
                    ));
                }
                let provider = by_id.get(upstream.as_str()).ok_or_else(|| {
                    TeamError::InvalidContract(
                        "V6 required dependency must resolve in the source-owned graph".into(),
                    )
                })?;
                if provider.workstream_id != entry.workstream_id
                    && !matches!(
                        entry.contract.dependency_acceptance.get(upstream),
                        Some(DependencyAcceptanceMode::CrossWorkstream(_))
                    )
                {
                    return Err(TeamError::InvalidContract("V6 cross-stream required dependency needs an explicit cross_workstream policy".into()));
                }
                edges.push((entry.contract.work_id.as_str().to_owned(), upstream.clone()));
            }
        }
        crate::planning::detect_cycle(
            &by_id.keys().map(|id| (*id).to_owned()).collect::<Vec<_>>(),
            &edges,
        )
        .map_err(|e| TeamError::InvalidContract(e.to_string()))
    }

    /// A projection identity, not an individual work's contract hash. Preserve
    /// each nested V1 hash so migration cannot silently reinterpret old receipts.
    pub fn hash(&self) -> TeamResult<String> {
        self.validate(&self.catalog.project_id)?;
        let mut catalog = self.catalog.clone();
        catalog.workstreams.sort_by_key(|s| s.id);
        for stream in &mut catalog.workstreams {
            stream.goal_keys.sort();
            stream.acceptance_contracts.sort();
        }
        let mut contracts = self
            .contracts
            .iter()
            .map(|e| {
                Ok((
                    e.contract.work_id.as_str(),
                    e.workstream_id,
                    e.contract.hash()?,
                ))
            })
            .collect::<TeamResult<Vec<_>>>()?;
        contracts.sort();
        contract_hash(&json!({"codec":self.codec,"catalog":catalog,"contracts":contracts}))
    }
}
