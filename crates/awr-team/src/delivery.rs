//! Provider-neutral delivery descriptions. Validation is not authorization.
//!
//! These records neither perform repository effects nor establish evidence
//! trust, artifact accessibility or approval. Authenticated stores must resolve
//! their references against the existing AWR review/completion rules.
mod binding;
mod records;

pub use binding::*;
pub use records::*;

use crate::{TeamError, TeamResult};
use serde::{Deserialize, Serialize};

pub const DELIVERY_PROTOCOL: &str = "awr-delivery";
pub const DELIVERY_PROTOCOL_VERSION: u32 = 1;
pub const MAX_DELIVERY_RECORD_BYTES: usize = 65536;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryEnvelope {
    pub protocol: String,
    pub protocol_version: u32,
    pub record: DeliveryRecord,
}

impl DeliveryEnvelope {
    pub fn validate(&self) -> TeamResult<()> {
        if self.protocol != DELIVERY_PROTOCOL || self.protocol_version != DELIVERY_PROTOCOL_VERSION
        {
            return Err(TeamError::ProtocolUnsupported);
        }
        self.record.validate()?;
        if serde_json::to_vec(self)
            .map_err(|_| TeamError::InvalidInput("delivery encoding failed".into()))?
            .len()
            > MAX_DELIVERY_RECORD_BYTES
        {
            return Err(TeamError::InvalidInput(
                "delivery record is too large".into(),
            ));
        }
        Ok(())
    }
}

pub fn parse_delivery_record(bytes: &[u8]) -> TeamResult<DeliveryEnvelope> {
    if bytes.len() > MAX_DELIVERY_RECORD_BYTES {
        return Err(TeamError::InvalidInput(
            "delivery record is too large".into(),
        ));
    }
    // Do not echo source values in malformed-input diagnostics.
    let envelope: DeliveryEnvelope = serde_json::from_slice(bytes)
        .map_err(|_| TeamError::InvalidInput("malformed delivery record".into()))?;
    envelope.validate()?;
    Ok(envelope)
}
