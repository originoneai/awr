//! Planning change writeback into authoritative source (AWR-TMCP-022).
//!
//! Source definition fields and coordinator runtime fields have separate write
//! authorities. Claim/session/execution/completion facts never enter source
//! bytes from this path; any compatible status note is derived only from a
//! verified domain result and never becomes a completion receipt.
use crate::locator::fingerprint;
use crate::publish_prep::source_status_notes_are_completion_receipts;
use awr_core::{Error, Result};
use awr_team::{DraftChange, DraftOpKind, TaskDraft};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Fields that may be written through the source/planning authority.
pub const SOURCE_WRITABLE_FIELDS: &[&str] = &[
    "title",
    "goals",
    "scope_paths",
    "acceptance",
    "required_dependencies",
    "completion_policy",
    "dependency_acceptance",
    "definition_state",
    "workstream",
    "split_from",
    "split_children",
    "external_key",
];

/// Fields owned exclusively by the coordinator runtime. Source writeback must
/// refuse to set these; exports cannot overwrite real runtime state.
pub const RUNTIME_ONLY_FIELDS: &[&str] = &[
    "claim_id",
    "session_id",
    "execution_id",
    "lease_state",
    "work_runtime_state",
    "completion_receipt_id",
    "execution_state",
    "actor_id",
    "client_id",
    "fence",
    "coordinator_epoch",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldWriteAuthority {
    Source,
    Runtime,
}

/// Unique write entry for source-definition fields.
pub fn source_field_authority(field: &str) -> Option<FieldWriteAuthority> {
    if RUNTIME_ONLY_FIELDS.contains(&field) {
        return Some(FieldWriteAuthority::Runtime);
    }
    if SOURCE_WRITABLE_FIELDS.contains(&field) || field == "status" {
        return Some(FieldWriteAuthority::Source);
    }
    None
}

/// Unique write entry for runtime/coordinator fields.
pub fn runtime_field_authority(field: &str) -> Option<FieldWriteAuthority> {
    if RUNTIME_ONLY_FIELDS.contains(&field) {
        Some(FieldWriteAuthority::Runtime)
    } else {
        None
    }
}

pub fn refuse_runtime_field_in_source_write(field: &str) -> Result<()> {
    if runtime_field_authority(field).is_some() {
        return Err(Error::InvalidInput(format!(
            "runtime field '{field}' cannot be written through source authority"
        )));
    }
    Ok(())
}

/// Compatible status writeback may only be derived from a verified domain
/// result. Source status / export files never overwrite completion receipts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedDomainStatus {
    pub work_external_key: String,
    pub domain_result: String,
    pub verified: bool,
    pub receipt_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompatibleStatusWriteback {
    pub work_external_key: String,
    pub source_status: String,
    pub derived_from_receipt: String,
}

pub fn derive_compatible_status_writeback(
    verified: &VerifiedDomainStatus,
) -> Result<CompatibleStatusWriteback> {
    if !verified.verified {
        return Err(Error::InvalidInput(
            "compatible status writeback requires a verified domain result".into(),
        ));
    }
    let receipt = verified.receipt_id.as_deref().unwrap_or("").trim();
    if receipt.is_empty() {
        return Err(Error::InvalidInput(
            "compatible status writeback requires a verified receipt id".into(),
        ));
    }
    if source_status_notes_are_completion_receipts() {
        return Err(Error::InvalidInput(
            "source status notes must never act as completion receipts".into(),
        ));
    }
    // Map verified domain outcomes to source vocabulary only; never invent done.
    let source_status = match verified.domain_result.as_str() {
        "accepted" | "complete_receipt_recorded" => "completed_in_source_note",
        "rework_required" => "needs_rework",
        other if !other.is_empty() => other,
        _ => {
            return Err(Error::InvalidInput(
                "verified domain result required for status writeback".into(),
            ));
        }
    };
    Ok(CompatibleStatusWriteback {
        work_external_key: verified.work_external_key.clone(),
        source_status: source_status.into(),
        derived_from_receipt: receipt.into(),
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerWritebackPatch {
    pub before_fingerprint: String,
    pub after_fingerprint: String,
    pub before_bytes: Vec<u8>,
    pub after_bytes: Vec<u8>,
    pub changed_external_keys: Vec<String>,
    pub refused_runtime_fields: Vec<String>,
}

/// Apply approved planning draft changes onto YAML ledger bytes as a precise
/// patch. Runtime-only fields in after-drafts are refused (not silently dropped
/// into source). Status keys in the ledger are left untouched unless a separate
/// verified compatible writeback is supplied by the caller.
pub fn apply_planning_changes_to_ledger(
    ledger_bytes: &[u8],
    changes: &[DraftChange],
) -> Result<LedgerWritebackPatch> {
    let before_fingerprint = fingerprint(ledger_bytes);
    let mut doc: Value = serde_yaml_ng::from_slice(ledger_bytes)
        .map_err(|e| Error::InvalidInput(format!("invalid YAML ledger for writeback: {e}")))?;
    let mut refused = Vec::new();
    let mut changed = BTreeSet::new();

    let items = doc
        .get_mut("work_items")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| Error::InvalidInput("ledger work_items required for writeback".into()))?;

    for change in changes {
        change
            .after
            .validate()
            .map_err(|e| Error::InvalidInput(e.to_string()))?;
        inspect_draft_for_runtime_fields(&change.after, &mut refused)?;
        match change.op {
            DraftOpKind::CreateTask => {
                let ws = change
                    .after
                    .workstream
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                if ws.is_none() {
                    return Err(Error::InvalidInput(format!(
                        "create_task {} requires workstream ownership before source writeback",
                        change.after.external_key
                    )));
                }
                if items.iter().any(|row| {
                    row.get("id").and_then(|v| v.as_str())
                        == Some(change.after.external_key.as_str())
                        || row.get("id").and_then(|v| v.as_str())
                            == Some(change.after.work_id.as_str())
                }) {
                    return Err(Error::SourceConflict(format!(
                        "create would overwrite existing work {}",
                        change.after.external_key
                    )));
                }
                items.push(draft_to_ledger_row(&change.after));
                changed.insert(change.after.external_key.clone());
            }
            DraftOpKind::EditFields
            | DraftOpKind::Split
            | DraftOpKind::Cancel
            | DraftOpKind::Archive => {
                let key = change.after.external_key.as_str();
                let Some(row) = items.iter_mut().find(|row| {
                    row.get("id").and_then(|v| v.as_str()) == Some(key)
                        || row.get("id").and_then(|v| v.as_str())
                            == Some(change.after.work_id.as_str())
                }) else {
                    return Err(Error::SourceConflict(format!(
                        "edit target {key} missing from authoritative ledger"
                    )));
                };
                // Preserve identity and any runtime-looking keys that already
                // exist only if they are true source vocabulary (e.g. status).
                let source_modes: Option<BTreeMap<String, awr_team::DependencyAcceptanceMode>> =
                    row.get("dependency_acceptance")
                        .map(|modes| serde_json::from_value(modes.clone()))
                        .transpose()
                        .map_err(|_| {
                            Error::InvalidInput("invalid source dependency_acceptance".into())
                        })?;
                let before_modes = change
                    .before
                    .as_ref()
                    .and_then(|b| b.dependency_acceptance.as_ref());
                // A V1 omission can retain an existing source policy. An
                // explicit replacement or prior map must match the source,
                // including absence, so an invented before-map cannot pass.
                if (change.after.dependency_acceptance.is_some() || before_modes.is_some())
                    && before_modes != source_modes.as_ref()
                {
                    return Err(Error::SourceConflict(
                        "include the exact prior dependency_acceptance before replacing it".into(),
                    ));
                }
                if let Some(map) = &source_modes {
                    if change.after.dependency_acceptance.is_none()
                        && map
                            .keys()
                            .any(|id| !change.after.required_dependencies.contains(id))
                    {
                        return Err(Error::InvalidInput("planning cannot orphan retained dependency_acceptance; use an explicit reviewed V2 policy edit".into()));
                    }
                }
                let policy = row
                    .get("completion_policy")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or(crate::publish_prep::DEFAULT_COMPLETION_POLICY);
                if change
                    .before
                    .as_ref()
                    .is_some_and(|b| b.completion_policy != policy)
                {
                    return Err(Error::SourceConflict(
                        "draft completion_policy does not match the authoritative source".into(),
                    ));
                }
                awr_team::ensure_independent_review_not_downgraded(
                    policy,
                    &change.after.completion_policy,
                )
                .map_err(|e| Error::RuleViolation(e.to_string()))?;
                apply_draft_fields(row, &change.after);
                // Never copy runtime-only keys into the row.
                for field in RUNTIME_ONLY_FIELDS {
                    if let Some(obj) = row.as_object_mut() {
                        obj.remove(*field);
                    }
                }
                changed.insert(change.after.external_key.clone());
            }
        }
    }

    if !refused.is_empty() {
        return Err(Error::InvalidInput(format!(
            "runtime fields refused in source writeback: {}",
            refused.join(",")
        )));
    }

    let after_bytes = serde_yaml_ng::to_string(&doc)
        .map_err(|e| Error::InvalidInput(format!("cannot serialize writeback ledger: {e}")))?
        .into_bytes();
    let after_fingerprint = fingerprint(&after_bytes);
    Ok(LedgerWritebackPatch {
        before_fingerprint,
        after_fingerprint,
        before_bytes: ledger_bytes.to_vec(),
        after_bytes,
        changed_external_keys: changed.into_iter().collect(),
        refused_runtime_fields: refused,
    })
}

fn inspect_draft_for_runtime_fields(draft: &TaskDraft, refused: &mut Vec<String>) -> Result<()> {
    // TaskDraft schema itself excludes runtime fields; guard the serialized form
    // so future extensions cannot smuggle coordinator facts into source.
    let value = serde_json::to_value(draft)
        .map_err(|e| Error::InvalidInput(format!("draft serialize: {e}")))?;
    if let Some(obj) = value.as_object() {
        for key in obj.keys() {
            if RUNTIME_ONLY_FIELDS.contains(&key.as_str()) {
                refused.push(key.clone());
            }
        }
    }
    Ok(())
}

fn draft_to_ledger_row(draft: &TaskDraft) -> Value {
    let mut row = json!({
        "id": draft.external_key,
        "title": draft.title,
        "goals": draft.goals,
        "acceptance": draft.acceptance,
        "paths": draft.scope_paths,
        "depends_on": draft.required_dependencies,
        "completion_policy": draft.completion_policy,
        "status": match draft.definition_state {
            awr_team::DraftDefinitionState::Draft => "planned",
            awr_team::DraftDefinitionState::Enabled => "planned",
            awr_team::DraftDefinitionState::Archived => "archived",
            awr_team::DraftDefinitionState::Cancelled => "cancelled",
        },
    });
    if let Some(obj) = row.as_object_mut() {
        if let Some(modes) = &draft.dependency_acceptance {
            obj.insert("dependency_acceptance".into(), json!(modes));
        }
        if let Some(ws) = draft
            .workstream
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            obj.insert("workstream".into(), json!(ws));
        }
        if let Some(parent) = &draft.split_from {
            obj.insert("split_from".into(), json!(parent));
        }
        if !draft.split_children.is_empty() {
            obj.insert("split_children".into(), json!(draft.split_children));
        }
    }
    row
}

fn apply_draft_fields(row: &mut Value, draft: &TaskDraft) {
    if let Some(obj) = row.as_object_mut() {
        obj.insert("title".into(), json!(draft.title));
        obj.insert("goals".into(), json!(draft.goals));
        obj.insert("acceptance".into(), json!(draft.acceptance));
        obj.insert("paths".into(), json!(draft.scope_paths));
        obj.insert("depends_on".into(), json!(draft.required_dependencies));
        obj.insert("completion_policy".into(), json!(draft.completion_policy));
        if let Some(modes) = &draft.dependency_acceptance {
            obj.insert("dependency_acceptance".into(), json!(modes));
        }
        // Preserve existing workstream ownership unless the draft explicitly
        // carries a non-empty workstream (CreateTask always does).
        if let Some(ws) = draft
            .workstream
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            obj.insert("workstream".into(), json!(ws));
        }
        // Intentionally do not overwrite ledger `status` from definition_state
        // alone: status writeback requires verified domain derivation.
        if let Some(parent) = &draft.split_from {
            obj.insert("split_from".into(), json!(parent));
        }
        if !draft.split_children.is_empty() {
            obj.insert("split_children".into(), json!(draft.split_children));
        }
        match draft.definition_state {
            awr_team::DraftDefinitionState::Archived => {
                obj.insert("status".into(), json!("archived"));
            }
            awr_team::DraftDefinitionState::Cancelled => {
                obj.insert("status".into(), json!("cancelled"));
            }
            _ => {}
        }
    }
}

/// Refuse installing writeback when on-disk bytes no longer match the planned
/// before fingerprint (external edit / concurrent writer).
pub fn refuse_external_overwrite(planned_before: &str, observed_before: &str) -> Result<()> {
    if planned_before != observed_before {
        return Err(Error::SourceConflict(
            "authoritative source changed externally; refusing overwrite of others' work".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use awr_team::{DraftDefinitionState, DraftOpKind, TaskDraft};

    fn draft(id: &str, deps: &[&str]) -> TaskDraft {
        TaskDraft {
            work_id: id.into(),
            external_key: id.into(),
            title: format!("Task {id}"),
            goals: vec!["delivery".into()],
            scope_paths: vec!["specs/a.md".into()],
            acceptance: vec!["ok".into()],
            required_dependencies: deps.iter().map(|s| (*s).into()).collect(),
            completion_policy: "independent_review".into(),
            dependency_acceptance: None,
            definition_state: DraftDefinitionState::Enabled,
            workstream: None,
            split_from: None,
            split_children: vec![],
        }
    }

    #[test]
    fn source_and_runtime_fields_have_separate_authorities() {
        assert_eq!(
            source_field_authority("title"),
            Some(FieldWriteAuthority::Source)
        );
        assert_eq!(
            runtime_field_authority("claim_id"),
            Some(FieldWriteAuthority::Runtime)
        );
        assert!(refuse_runtime_field_in_source_write("execution_id").is_err());
        assert!(refuse_runtime_field_in_source_write("title").is_ok());
        assert!(!source_status_notes_are_completion_receipts());
    }

    #[test]
    fn compatible_status_requires_verified_receipt() {
        assert!(
            derive_compatible_status_writeback(&VerifiedDomainStatus {
                work_external_key: "API-1".into(),
                domain_result: "accepted".into(),
                verified: false,
                receipt_id: Some("r1".into()),
            })
            .is_err()
        );
        let ok = derive_compatible_status_writeback(&VerifiedDomainStatus {
            work_external_key: "API-1".into(),
            domain_result: "accepted".into(),
            verified: true,
            receipt_id: Some("r1".into()),
        })
        .unwrap();
        assert_eq!(ok.derived_from_receipt, "r1");
        assert_ne!(ok.source_status, "done");
    }

    #[test]
    fn ledger_patch_updates_deps_and_fingerprints() {
        let ledger = br#"workstreams:
  version: 1
  definitions: []
goals: []
work_items:
  - id: API-1
    title: Define
    status: completed
    goals: [delivery]
    acceptance: [a]
    paths: [openapi.yaml]
    depends_on: []
  - id: CLIENT-1
    title: SDK
    status: planned
    goals: [delivery]
    acceptance: [b]
    paths: [sdk/]
    depends_on: [API-1]
"#;
        let mut after = draft("CLIENT-1", &["API-1", "SHARED-1"]);
        after.title = "SDK revised".into();
        let patch = apply_planning_changes_to_ledger(
            ledger,
            &[DraftChange {
                op: DraftOpKind::EditFields,
                before: Some(draft("CLIENT-1", &["API-1"])),
                after,
            }],
        )
        .unwrap();
        assert_ne!(patch.before_fingerprint, patch.after_fingerprint);
        assert!(patch.changed_external_keys.contains(&"CLIENT-1".into()));
        let text = String::from_utf8(patch.after_bytes.clone()).unwrap();
        assert!(text.contains("SHARED-1"));
        assert!(text.contains("SDK revised"));
        // Existing source status preserved on edit.
        assert!(text.contains("status: planned") || text.contains("status:planned"));
    }

    #[test]
    fn external_fingerprint_mismatch_refuses_overwrite() {
        assert!(refuse_external_overwrite("sha256:a", "sha256:b").is_err());
        assert!(refuse_external_overwrite("sha256:a", "sha256:a").is_ok());
    }

    #[test]
    fn v1_planning_preserves_existing_dependency_policy_and_refuses_orphaning() {
        let ledger = br#"work_items:
  - id: CLIENT-1
    title: SDK
    depends_on: [API-1]
    dependency_acceptance:
      API-1: agent_reviewed_caller_asserted_reconciled
"#;
        let before = draft("CLIENT-1", &["API-1"]);
        let mut after = before.clone();
        after.title = "Updated SDK".into();
        let change = DraftChange {
            op: DraftOpKind::EditFields,
            before: Some(before),
            after,
        };
        let patch = apply_planning_changes_to_ledger(ledger, &[change.clone()]).unwrap();
        let parsed: Value = serde_yaml_ng::from_slice(&patch.after_bytes).unwrap();
        assert_eq!(
            parsed["work_items"][0]["dependency_acceptance"]["API-1"],
            "agent_reviewed_caller_asserted_reconciled"
        );
        assert_eq!(parsed["work_items"][0]["title"], "Updated SDK");
        let mut orphan = change;
        orphan.after.required_dependencies.clear();
        assert!(
            apply_planning_changes_to_ledger(ledger, &[orphan])
                .unwrap_err()
                .to_string()
                .contains("cannot orphan")
        );
    }

    fn modes(ids: &[&str]) -> BTreeMap<String, awr_team::DependencyAcceptanceMode> {
        ids.iter()
            .map(|id| {
                (
                    (*id).into(),
                    awr_team::DependencyAcceptanceMode::AgentReviewedCallerAssertedReconciled,
                )
            })
            .collect()
    }

    #[test]
    fn create_writes_reviewed_completion_and_dependency_policies() {
        let mut task = draft("CLIENT-1", &["API-1"]);
        task.workstream = Some("client".into());
        task.completion_policy = "caller_managed_execution_and_agent_review".into();
        task.dependency_acceptance = Some(modes(&["API-1"]));
        let patch = apply_planning_changes_to_ledger(
            b"work_items: []\n",
            &[DraftChange {
                op: DraftOpKind::CreateTask,
                before: None,
                after: task,
            }],
        )
        .unwrap();
        let parsed: Value = serde_yaml_ng::from_slice(&patch.after_bytes).unwrap();
        assert_eq!(
            parsed["work_items"][0]["completion_policy"],
            "caller_managed_execution_and_agent_review"
        );
        assert_eq!(
            parsed["work_items"][0]["dependency_acceptance"]["API-1"],
            "agent_reviewed_caller_asserted_reconciled"
        );
        assert_eq!(parsed["work_items"][0]["status"], "planned");
    }

    #[test]
    fn edits_retain_agent_review_and_require_exact_prior_dependency_map() {
        let ledger = br#"work_items:
  - id: CLIENT-1
    status: planned
    depends_on: [API-1]
    completion_policy: caller_managed_execution_and_agent_review
    dependency_acceptance:
      API-1: agent_reviewed_caller_asserted_reconciled
"#;
        let mut before = draft("CLIENT-1", &["API-1"]);
        before.completion_policy = "caller_managed_execution_and_agent_review".into();
        before.dependency_acceptance = Some(modes(&["API-1"]));
        let mut after = before.clone();
        after.title = "Revised SDK".into();
        after.required_dependencies = vec!["SHARED-1".into()];
        after.dependency_acceptance = Some(modes(&["SHARED-1"]));
        let change = DraftChange {
            op: DraftOpKind::EditFields,
            before: Some(before),
            after,
        };
        let patch = apply_planning_changes_to_ledger(ledger, &[change.clone()]).unwrap();
        let parsed: Value = serde_yaml_ng::from_slice(&patch.after_bytes).unwrap();
        assert_eq!(
            parsed["work_items"][0]["completion_policy"],
            "caller_managed_execution_and_agent_review"
        );
        assert_eq!(
            parsed["work_items"][0]["dependency_acceptance"],
            json!(modes(&["SHARED-1"]))
        );
        let mut stale = change;
        stale.before.as_mut().unwrap().dependency_acceptance = None;
        assert!(matches!(
            apply_planning_changes_to_ledger(ledger, &[stale]),
            Err(Error::SourceConflict(_))
        ));
    }

    #[test]
    fn invented_prior_dependency_policy_is_rejected_even_when_after_omits_it() {
        let ledger = b"work_items:\n  - id: CLIENT-1\n    depends_on: [API-1]\n";
        let mut before = draft("CLIENT-1", &["API-1"]);
        before.dependency_acceptance = Some(modes(&["API-1"]));
        let change = DraftChange {
            op: DraftOpKind::EditFields,
            before: Some(before),
            after: draft("CLIENT-1", &["API-1"]),
        };
        assert!(matches!(
            apply_planning_changes_to_ledger(ledger, &[change]),
            Err(Error::SourceConflict(_))
        ));
    }

    #[test]
    fn source_completion_policy_rejects_stale_before_and_independent_review_downgrade() {
        // Blank and absent policies compile to the independent-review default.
        for source_policy in [
            "",
            "    completion_policy: '  '\n",
            "    completion_policy: independent_review\n",
        ] {
            let ledger = format!("work_items:\n  - id: CLIENT-1\n{source_policy}");
            let mut after = draft("CLIENT-1", &[]);
            after.completion_policy = "caller_managed_execution_and_agent_review".into();
            let mut change = DraftChange {
                op: DraftOpKind::EditFields,
                before: Some(draft("CLIENT-1", &[])),
                after,
            };
            assert!(matches!(
                apply_planning_changes_to_ledger(ledger.as_bytes(), &[change.clone()]),
                Err(Error::RuleViolation(_))
            ));
            change.before.as_mut().unwrap().completion_policy =
                "caller_managed_execution_and_agent_review".into();
            assert!(matches!(
                apply_planning_changes_to_ledger(ledger.as_bytes(), &[change]),
                Err(Error::SourceConflict(_))
            ));
        }
    }
}
