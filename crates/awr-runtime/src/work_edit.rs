//! Small field-edit facade over the existing reviewed host-save journal.
use crate::{HostChange, HostSaveReport, HostSaveRequest, host_preview, host_save};
use awr_core::*;
use awr_store::Store;
use serde_json::{Map, json};
use std::path::Path;

pub fn edit_work(
    store: &mut Store,
    root: &Path,
    mut request: HostSaveRequest,
    accept: Option<(Revision, &str)>,
) -> Result<HostSaveReport> {
    let HostChange::Fields {
        kind: EntityKind::WorkItem,
        target,
        source_fingerprint,
        fields,
    } = &mut request.change
    else {
        return Err(Error::InvalidInput("work edit requires work fields".into()));
    };
    let changes = fields
        .as_object()
        .ok_or_else(|| Error::InvalidInput("work edit fields must be an object".into()))?;
    if changes.is_empty()
        || changes.iter().any(|(k, v)| match k.as_str() {
            "title" | "summary" | "priority" | "next_action" | "kind" | "milestone" => {
                !v.is_string()
            }
            "acceptance" | "goal" => !v.as_array().is_some_and(|values| {
                !values.is_empty()
                    && values
                        .iter()
                        .all(|value| value.as_str().is_some_and(|s| !s.trim().is_empty()))
            }),
            _ => true,
        })
    {
        return Err(Error::MutationUnsupported("work edit accepts title, summary, priority and next_action text, and draft acceptance, goal, kind and milestone fields; use dedicated work actions for lifecycle/ownership/completion and reviewed batch changes for other fields".into()));
    }
    if request.actor.origin != HostEditOrigin::DelegatedAgent {
        return Err(Error::RuleViolation(
            "work edit records delegated-agent provenance and always requires a reviewed preview"
                .into(),
        ));
    }
    if accept.is_some() && source_fingerprint.is_empty() {
        return Err(Error::InvalidInput(
            "work edit apply requires the source_fingerprint returned by preview".into(),
        ));
    }
    let review = if accept.is_none() {
        let project = store.project_by_root(root)?;
        let work = store.work_item(project.id, target)?;
        if changes
            .keys()
            .any(|key| matches!(key.as_str(), "acceptance" | "goal" | "kind" | "milestone"))
            && work.item.status != WorkStatus::Draft
        {
            return Err(Error::RuleViolation(
                "planning-field repairs through work edit require a draft".into(),
            ));
        }
        if source_fingerprint.is_empty() {
            *source_fingerprint = work.source.fingerprint.clone();
        }
        let source = json!({"locator":work.item.meta.source_ref.locator,"pointer":work.item.meta.source_ref.pointer,"fingerprint":source_fingerprint});
        let current = serde_json::to_value(&work.item)?;
        let before: Map<_, _> = changes
            .keys()
            .map(|k| (k.clone(), current[k].clone()))
            .collect();
        Some(json!({"source":source,"before":before,"after":fields}))
    } else {
        // A replay must reach the existing journal even if the task was later removed.
        None
    };
    let change = request.change.clone();
    let key = request.request_key.clone();
    let mut report = match accept {
        Some((revision, preview)) => host_save(store, root, request, revision, Some(preview))?,
        None => host_preview(store, root, request)?,
    };
    if let Some(review) = review {
        report.value["preview_fingerprint"] = report.value["preview"]["fingerprint"].clone();
        report.value["review"] = review;
        report.value["change"] = json!(change);
        report.value.as_object_mut().unwrap().remove("preview");
    } else {
        // The durable journal retains the full proposal; don't repeat source bodies.
        report.value.as_object_mut().unwrap().remove("outcome");
    }
    report.value["request_key"] = json!(key);
    report.value["view"] = json!("work_edit");
    report.value["guidance"] = if accept.is_none() {
        json!({"when":"The displayed field edits match the requested change","basis":"Reviewed source fingerprint, project revision and preserved unrelated source facts","next_action":"Apply the same request with the returned source_fingerprint, preview_fingerprint and project_revision","recheck":"Any source change, revision conflict or uncertain write result; query host status before retrying"})
    } else {
        json!({"when":"The edit has returned an outcome","basis":"Existing durable host-save receipt","next_action":"Use the receipt status; inspect host status or explicitly recover pending work","recheck":"Pending recovery, external source changes or a new edit"})
    };
    Ok(report)
}
