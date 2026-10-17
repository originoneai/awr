//! Explicit, source-owned specification declarations and exact task adoption refs.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequirementDisposition {
    Required,
    Future,
    Cancelled,
    OutOfScope,
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpecificationItem {
    pub id: String,
    pub disposition: RequirementDisposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
}

/// Lives in a registered decision document, governed by its existing approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Specification {
    pub version: String,
    pub items: Vec<SpecificationItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpecificationReference {
    /// Stable external decision key, never an arbitrary file to open.
    pub specification: String,
    pub item: String,
    pub approved_version: String,
    /// Exact bytes adopted by the task, including decision lifecycle metadata.
    pub source_fingerprint: String,
    /// Exact current acceptance text, or an explicitly work-wide association.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criterion: Option<String>,
}

fn name(value: &str, max: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= max
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

impl Specification {
    pub fn validate(&self) -> Result<()> {
        let mut ids = BTreeSet::new();
        if !name(&self.version, 256)
            || self.items.is_empty()
            || self.items.len() > 1000
            || self.items.iter().any(|item| {
                !name(&item.id, 512)
                    || !ids.insert(&item.id)
                    || item.superseded_by.as_ref().is_some_and(|s| !name(s, 512))
                    || (item.superseded_by.is_some()
                        && item.disposition != RequirementDisposition::Superseded)
            })
        {
            return Err(Error::InvalidInput("specification requires a version, 1..1000 uniquely named items and explicit dispositions; superseded_by belongs only to superseded items".into()));
        }
        Ok(())
    }
}

pub fn validate_specification_references(refs: &[SpecificationReference]) -> Result<()> {
    let mut seen = BTreeSet::new();
    if refs.len() > 1000
        || refs.iter().any(|r| {
            !name(&r.specification, 512)
                || !name(&r.item, 512)
                || !name(&r.approved_version, 256)
                || !r
                    .source_fingerprint
                    .strip_prefix("sha256:")
                    .is_some_and(|s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()))
                || r.criterion
                    .as_ref()
                    .is_some_and(|c| c.trim().is_empty() || c.len() > 16384)
                || !seen.insert((
                    &r.specification,
                    &r.item,
                    &r.approved_version,
                    &r.source_fingerprint,
                    &r.criterion,
                ))
        })
    {
        return Err(Error::InvalidInput("spec_refs requires at most 1000 distinct references with specification, item, approved_version, sha256 source_fingerprint and optional exact criterion text".into()));
    }
    Ok(())
}
