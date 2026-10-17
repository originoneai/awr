//! Explicit source vocabulary. Mapping changes are bound to the source revision.
use crate::SourceSpec;
use awr_core::{Error, Result, WorkStatus};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const FIELDS: &[&str] = &[
    "id",
    "title",
    "status",
    "kind",
    "owner",
    "priority",
    "required",
    "summary",
    "next_action",
    "blocker",
    "acceptance",
    "spec_refs",
    "depends_on",
    "goal",
    "milestone",
    "tags",
    "paths",
];

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LedgerMapping {
    pub ordinary_work_policy: Option<awr_core::OrdinaryWorkPolicy>,
    /// Canonical work field -> original YAML key or Markdown column.
    pub field_map: BTreeMap<String, String>,
    /// Original status -> canonical work status. Never guesses unknown states.
    pub status_map: BTreeMap<String, String>,
}
impl LedgerMapping {
    pub fn from_spec(spec: &SourceSpec) -> Result<Self> {
        let mut mapping: Self = toml::Value::Table(spec.options.clone())
            .try_into()
            .map_err(|_| {
                Error::InvalidInput(
                    "ledger options require field_map and/or status_map string tables".into(),
                )
            })?;
        let mut names = BTreeSet::new();
        if let Some(policy) = &mapping.ordinary_work_policy {
            policy.validate()?;
        }
        if mapping.field_map.len() > FIELDS.len() || mapping.status_map.len() > 128 {
            return Err(Error::InvalidInput(
                "ledger mapping exceeds its field/status limit".into(),
            ));
        }
        for (field, original) in &mapping.field_map {
            if !FIELDS.contains(&field.as_str())
                || original.trim().is_empty()
                || original.len() > 128
                || original != original.trim()
                || original.chars().any(char::is_control)
                || !names.insert(original.to_lowercase())
                || (FIELDS.contains(&original.as_str()) && original != field)
                || [
                    "external_key",
                    "required_for_v1",
                    "dependencies",
                    "goals",
                    "deliverables",
                    "verification",
                    "evidence",
                ]
                .contains(&original.as_str())
            {
                return Err(Error::InvalidInput("field_map must use supported work fields and distinct, non-conflicting source names".into()));
            }
            if spec.adapter == "markdown-ledger-v1"
                && ![
                    "id",
                    "title",
                    "status",
                    "owner",
                    "priority",
                    "next_action",
                    "acceptance",
                    "depends_on",
                    "goal",
                ]
                .contains(&field.as_str())
            {
                return Err(Error::InvalidInput(
                    "field_map selects a field not supported by the Markdown ledger".into(),
                ));
            }
        }
        let mut statuses = BTreeMap::new();
        let mut keys = BTreeSet::new();
        for (raw, target) in mapping.status_map {
            let key = raw.trim().to_lowercase();
            let status = WorkStatus::normalize(&target);
            let existing = WorkStatus::normalize(&key);
            if key.is_empty()
                || raw.len() > 128
                || raw != raw.trim()
                || raw.chars().any(char::is_control)
                || status == WorkStatus::Unknown
                // Before draft became a built-in nonselectable state, projects could
                // explicitly map that source spelling. Retain those prior mappings.
                || (existing != WorkStatus::Unknown && existing != WorkStatus::Draft && existing != status)
                || !keys.insert(key)
            {
                return Err(Error::InvalidInput("status_map requires unique source statuses and canonical targets; canonical states cannot be redefined".into()));
            }
            statuses.insert(raw, target);
        }
        mapping.status_map = statuses;
        Ok(mapping)
    }
    pub fn status(&self, raw: &str) -> WorkStatus {
        let key = raw.trim().to_lowercase();
        WorkStatus::normalize(
            self.status_map
                .iter()
                .find(|(name, _)| name.to_lowercase() == key)
                .map(|(_, v)| v.as_str())
                .unwrap_or(&key),
        )
    }
    pub fn source_field<'a>(&'a self, canonical: &'a str) -> &'a str {
        self.field_map
            .get(canonical)
            .map(String::as_str)
            .unwrap_or(canonical)
    }
    pub fn column<'a>(&'a self, original: &'a str) -> Option<&'a str> {
        self.field_map
            .iter()
            .find(|(_, name)| name.eq_ignore_ascii_case(original.trim()))
            .map(|(field, _)| field.as_str())
    }
    pub fn record(&self, original: &Value) -> Result<Value> {
        let mut value = original.clone();
        let record = value
            .as_object_mut()
            .ok_or_else(|| Error::InvalidInput("work record must be a mapping".into()))?;
        for (field, source) in &self.field_map {
            if field == source {
                continue;
            }
            if record.contains_key(field) {
                return Err(Error::SourceConflict(format!(
                    "mapped work field {field} also has a canonical source key; choose one authority"
                )));
            }
            if let Some(v) = record.remove(source) {
                record.insert(field.clone(), v);
            }
        }
        Ok(value)
    }
    pub fn document(&self, original: Value) -> Result<Value> {
        let mut document = original;
        match document.get_mut("work_items") {
            Some(Value::Array(records)) => {
                for record in records {
                    *record = self.record(record)?;
                }
            }
            Some(Value::Object(records)) => {
                for record in records.values_mut() {
                    *record = self.record(record)?;
                }
            }
            _ => (),
        }
        Ok(document)
    }
    pub fn write_value(&self, field: &str, value: &Value, original: &Value) -> Result<Value> {
        if field != "status" {
            return Ok(value.clone());
        }
        let desired = value
            .as_str()
            .ok_or_else(|| Error::InvalidInput("work status must be a string".into()))?;
        let desired_status = WorkStatus::normalize(desired);
        if desired_status == WorkStatus::Unknown {
            return Err(Error::InvalidInput(
                "work action requires a canonical status".into(),
            ));
        }
        if let Some(raw) = original[self.source_field("status")].as_str()
            && self.status(raw) == desired_status
        {
            return Ok(Value::String(raw.into()));
        }
        let aliases: Vec<_> = self
            .status_map
            .iter()
            .filter(|(_, v)| v.as_str() == desired)
            .map(|(raw, _)| raw)
            .collect();
        match aliases.as_slice() {
            [] => Ok(value.clone()),
            [raw] => Ok(Value::String((*raw).clone())),
            _ => Err(Error::MutationUnsupported("multiple source spellings map to the target status; retain one write spelling in status_map or edit the source explicitly".into())),
        }
    }
}
