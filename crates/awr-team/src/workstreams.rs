//! Explicit multi-work source codec. Work branches keep their existing scope ID;
//! workstream ownership is a separate, source-backed dimension.
use crate::{TeamError, TeamResult, WorkContract, contract_hash};
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

    pub fn validate(&self, project_id: &str) -> TeamResult<()> {
        if !matches!(
            self.codec.as_str(),
            Self::CODEC | Self::CODEC_V2 | Self::CODEC_V3 | Self::CODEC_V4 | Self::CODEC_V5
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
                    WorkContract::CODEC_V3 | WorkContract::CODEC_V4 | WorkContract::CODEC_V5
                ))
                || (self.codec == Self::CODEC_V3
                    && matches!(
                        entry.contract.codec.as_str(),
                        WorkContract::CODEC_V4 | WorkContract::CODEC_V5
                    ))
                || (self.codec == Self::CODEC_V4 && entry.contract.codec == WorkContract::CODEC_V5)
            {
                return Err(TeamError::InvalidContract(
                    "contract codec is newer than the workstream codec".into(),
                ));
            }
            for upstream in entry.contract.dependency_acceptance.keys() {
                if self
                    .contracts
                    .iter()
                    .find(|provider| provider.contract.work_id.as_str() == upstream)
                    .is_none_or(|provider| provider.workstream_id != entry.workstream_id)
                {
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
