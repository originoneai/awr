use crate::{
    Locator, ParseContext, SourceSnapshot, SourceSpec, fingerprint, inspect_mutation_source,
};
use awr_core::*;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub struct PreparedYamlMutation {
    pub path: PathBuf,
    pub before: SourceSnapshot,
    pub after: SourceSnapshot,
    pub plan: MutationWritePlan,
}

fn unsupported(message: &str) -> Error {
    Error::MutationUnsupported(message.into())
}
fn document(snapshot: &SourceSnapshot) -> Result<Value> {
    crate::limits::check_source_size(&snapshot.bytes, crate::YAML_READ_CAP)?;
    let yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(snapshot.text()?)
        .map_err(|e| Error::InvalidInput(format!("YAML mutation: {e}")))?;
    Ok(serde_json::to_value(yaml)?)
}

/// Read the exact current YAML record when constructing a preserving domain mutation.
pub fn read_yaml_mutation_record(
    root: &Path,
    source: &Source,
    patch: &MutationPatch,
) -> Result<Value> {
    if source.adapter == "markdown-ledger-v1" {
        return crate::read_markdown_mutation_record(root, source, patch);
    }
    crate::verify_mutation_source(root, source, patch)?;
    if source.adapter != "yaml-ledger-v1" {
        return Err(unsupported(
            "completion requires a writable YAML ledger source",
        ));
    }
    let (_, spec, snapshot) = inspect_mutation_source(root, source, patch)?;
    if snapshot.fingerprint != patch.target.meta.source_ref.source_fingerprint {
        return Err(Error::SourceConflict(
            "source changed while reading its record".into(),
        ));
    }
    let record = document(&snapshot)?
        .pointer(patch.target.meta.source_ref.pointer.as_deref().unwrap())
        .cloned()
        .ok_or_else(|| Error::SourceConflict("exact YAML record is missing".into()))?;
    if patch.target.kind == EntityKind::WorkItem {
        crate::LedgerMapping::from_spec(&spec)?.record(&record)
    } else {
        Ok(record)
    }
}
pub fn yaml_field_writable(kind: EntityKind, field: &str) -> bool {
    match kind {
        EntityKind::Goal => matches!(
            field,
            "title" | "status" | "priority" | "summary" | "success_criteria" | "acceptance"
        ),
        EntityKind::Plan => matches!(
            field,
            "title" | "name" | "status" | "summary" | "scope" | "acceptance" | "success_criteria"
        ),
        // Runtime domain actions will enable guarded work state/ownership changes. Generic
        // update_fields cannot become a back door around completion and evidence checks.
        EntityKind::WorkItem => matches!(
            field,
            "title"
                | "kind"
                | "priority"
                | "required"
                | "required_for_v1"
                | "summary"
                | "next_action"
                | "score"
                | "tags"
                | "paths"
                | "deliverables"
                | "acceptance"
                | "milestone"
                | "depends_on"
                | "dependencies"
                | "goal"
                | "goals"
        ),
        EntityKind::Evidence => field == "summary",
        _ => false,
    }
}
pub fn parse_mutation_projection(
    source: &Source,
    spec: &SourceSpec,
    snapshot: &SourceSnapshot,
    existing_ids: BTreeMap<(EntityKind, String), Id>,
    target: &MutationTarget,
) -> Result<(ProjectionBatch, String)> {
    let batch = crate::source_adapter(&source.adapter)?.parse(
        snapshot,
        &ParseContext {
            source,
            existing_ids,
        },
        spec,
    )?;
    let value = serde_json::to_value(&batch)?;
    let key = match target.kind {
        EntityKind::Goal => "goals",
        EntityKind::Plan => "plans",
        EntityKind::WorkItem => "work_items",
        EntityKind::Evidence => "evidence",
        _ => return Err(unsupported("this entity kind has no YAML writer")),
    };
    let rows = value[key].as_array().unwrap();
    let object = rows
        .iter()
        .find(|row| {
            row["id"] == serde_json::json!(target.meta.id)
                && row["external_key"] == target.meta.external_key
        })
        .ok_or_else(|| {
            Error::SourceConflict("source mutation changed the target identity".into())
        })?;
    Ok((batch, mutation_projection_hash(object)?))
}
pub fn prepare_yaml_mutation(
    root: &Path,
    source: &Source,
    proposal: &MutationProposal,
    existing_ids: BTreeMap<(EntityKind, String), Id>,
) -> Result<PreparedYamlMutation> {
    if source.adapter == "markdown-ledger-v1" {
        return crate::prepare_markdown_mutation(root, source, proposal, existing_ids);
    }
    let patch = proposal.bound_patch()?;
    if source.adapter != "yaml-ledger-v1" {
        return Err(unsupported("this source adapter has no automatic writer"));
    }
    let (locator, spec, before) = inspect_mutation_source(root, source, &patch)?;
    let Locator::File(path) = locator else {
        return Err(unsupported("Git sources remain read-only"));
    };
    if crate::is_runtime_owned_path(&root.canonicalize()?, &path) {
        return Err(unsupported(
            "runtime-owned files cannot be source mutation targets; relocate authored documents outside .awr (except registered .awr/intake sources) and reindex",
        ));
    }
    if before.fingerprint != proposal.base_fingerprint {
        return Err(Error::SourceConflict(
            "source changed before preparing YAML mutation".into(),
        ));
    }
    let changes = patch.changes.as_object().unwrap();
    if let Some(field) = changes
        .keys()
        .find(|field| !mutation_field_writable(&patch, field))
    {
        return Err(unsupported(&format!(
            "field {field} requires its domain action"
        )));
    }
    let pointer = patch.target.meta.source_ref.pointer.as_deref().unwrap();
    let original = document(&before)?;
    let mapping = if patch.target.kind == EntityKind::WorkItem {
        crate::LedgerMapping::from_spec(&spec)?
    } else {
        crate::LedgerMapping::default()
    };
    if let Some(binding) = patch
        .work_action
        .as_ref()
        .and_then(|a| a.completion.as_ref())
    {
        let record = original
            .pointer(pointer)
            .ok_or_else(|| unsupported("exact YAML record is missing"))?;
        if patch.changes != completion_source_changes(&mapping.record(record)?, binding)? {
            return Err(Error::InvalidInput("completion patch must preserve existing evidence and verification metadata exactly".into()));
        }
    }
    let mut expected = original.clone();
    let record = expected
        .pointer_mut(pointer)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| unsupported("the exact YAML record is not a mapping"))?;
    for (key, value) in changes {
        let original_record = original.pointer(pointer).unwrap();
        record.insert(
            mapping.source_field(key).into(),
            if patch.target.kind == EntityKind::WorkItem {
                mapping.write_value(key, value, original_record)?
            } else {
                value.clone()
            },
        );
    }
    if expected == original {
        return Err(Error::InvalidInput(
            "proposal would not change its source record".into(),
        ));
    }
    let changed_fields = expected
        .pointer(pointer)
        .unwrap()
        .as_object()
        .unwrap()
        .iter()
        .filter(|(key, value)| original.pointer(pointer).unwrap().get(*key) != Some(*value))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let output = crate::yaml_edit::edit_fields(before.text()?, pointer, &changed_fields)?;
    crate::limits::check_source_size(output.as_bytes(), crate::YAML_READ_CAP)?;
    let after = SourceSnapshot {
        content_review: None,
        locator: before.locator.clone(),
        fingerprint: fingerprint(output.as_bytes()),
        bytes: output.into_bytes(),
    };
    if document(&after)? != expected {
        return Err(unsupported(
            "exact field replacement cannot preserve the document's other facts",
        ));
    }
    let (_, target_after_hash) =
        parse_mutation_projection(source, &spec, &after, existing_ids, &patch.target)?;
    let plan = MutationWritePlan {
        id: Id::new(),
        before_fingerprint: before.fingerprint.clone(),
        after_fingerprint: after.fingerprint.clone(),
        before_size: before.bytes.len() as u64,
        after_size: after.bytes.len() as u64,
        target_after_hash,
    };
    plan.validate()?;
    Ok(PreparedYamlMutation {
        path,
        before,
        after,
        plan,
    })
}

pub(crate) fn mutation_field_writable(patch: &MutationPatch, field: &str) -> bool {
    !(patch.work_action.is_none()
        && !yaml_field_writable(patch.target.kind, field)
        && !(patch
            .host_edit
            .as_ref()
            .is_some_and(|h| h.action == HostEditAction::ActivateDraft)
            && field == "status")
        && !(patch
            .host_edit
            .as_ref()
            .is_some_and(|h| h.action == HostEditAction::ConfirmOrdinary)
            && matches!(field, "status" | "ordinary_completion")))
}
