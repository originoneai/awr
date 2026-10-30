use awr_core::*;
use awr_store::Store;
use serde_json::{Value, json};
use std::path::Path;

pub(crate) fn session_guard(
    store: &Store,
    project: Id,
    session: Id,
) -> Result<Option<ActionGuidance>> {
    let bound = store.session(project, session)?;
    if bound.status != "active" {
        return Ok(Some(ActionGuidance::new(
            "Selected session is no longer active",
            "Current session status",
            "Inspect its checkpoint and successor; use the existing resume flow",
            "Session resumed and fresh required context consumed",
        )));
    }
    if let Some(work) = bound.work_item_id {
        for execution in store
            .executions(project, Some(work))?
            .iter()
            .filter(|e| e.branch_id == bound.branch_id && !e.state.terminal())
        {
            let uncertain_external = if execution.intent.executor == ExecutorKind::External {
                store
                    .latest_external_report(project, execution.id)?
                    .is_some_and(|e| {
                        !matches!(
                            e.payload["report"]["phase"].as_str(),
                            Some("succeeded" | "failed")
                        )
                    })
            } else {
                false
            };
            if execution.started_at.is_some() || uncertain_external {
                return Ok(Some(ActionGuidance::new(
                    "A started execution has no terminal outcome",
                    "Current execution records for this work and branch",
                    "Query execution status and reconcile its result before retrying or handing off",
                    "A terminal result or explicit recovery decision is recorded",
                )));
            }
        }
    }
    if store
        .mcp_waits(project, session)?
        .iter()
        .any(|w| w.status == "waiting_user")
    {
        return Ok(Some(ActionGuidance::new(
            "A persistent user wait is open",
            "Current session wait records",
            "Wait for the user's reply; keep the checkpoint and do not repeat the request",
            "The wait is resolved with the user's actual reply",
        )));
    }
    Ok(None)
}

/// Apply an opt-in presentation on the same read snapshot as preparation.
/// Required context, mandatory actions, diagnostics and incomplete results stay intact.
pub fn guide_prepared_work(
    store: &Store,
    root: &Path,
    mut value: Value,
    session: Option<Id>,
) -> Result<Value> {
    let project = store.project_by_root(root)?;
    let guard = session
        .map(|s| session_guard(store, project.id, s))
        .transpose()?
        .flatten();
    let own_claim = session.is_some_and(|s| {
        value["active_claims"].as_array().is_some_and(|claims| {
            !claims.is_empty() && claims.iter().all(|c| c["session_id"] == json!(s))
        })
    });
    let ongoing = matches!(
        value["work"]["status"].as_str(),
        Some("planned" | "ready" | "in_progress" | "claimed")
    );
    let continuation_only = own_claim
        && ongoing
        && value["diagnostics"].as_array().is_some_and(|ds| {
            ds.iter().all(|d| {
                matches!(
                    d["code"].as_str(),
                    Some("status_not_selectable" | "active_claim")
                )
            })
        });
    let compaction = if let Some(s) = session {
        Some(crate::inspect_compaction(
            store,
            root,
            &crate::InspectCompactionRequest {
                session: s,
                include_observation: false,
            },
        )?)
    } else {
        None
    };
    // The earliest material gap of governed work, when it has one.
    let diagnostics: Vec<(String, String)> = value["diagnostics"]
        .as_array()
        .map(|ds| {
            ds.iter()
                .filter_map(|d| {
                    Some((
                        d["code"].as_str()?.to_string(),
                        d["detail"].as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let blocking: Vec<String> = value["findings"]["open_blocking"]
        .as_array()
        .map(|keys| {
            keys.iter()
                .filter_map(|k| k.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let governance = governance_guidance(&GovernanceView {
        diagnostics: &diagnostics,
        open_blocking_findings: &blocking,
    });
    let guidance = if let Some(guard) = guard {
        guard
    } else if value["context"]["completeness"]["complete"] != true {
        ActionGuidance::new(
            "Required context is incomplete",
            "context.completeness.issues and source freshness",
            "Resolve the listed required gaps for this work, then prepare again",
            "Required sources, rules or work contract change",
        )
    } else if let Some(governance) = governance {
        governance
    } else if value["ready"] != true && !continuation_only {
        ActionGuidance::new(
            "Work has readiness blockers",
            "diagnostics and active_claims in this response",
            "Resolve this work's blockers or conflicting claim; do not repair unrelated ledger items",
            "Relevant source, dependency or claim changes",
        )
    } else if let Some(c) = compaction
        .as_ref()
        .filter(|c| c["state"] == "handoff_candidate")
    {
        serde_json::from_value(c["guidance"].clone())?
    } else if value["management"]["record_required"] == true {
        ActionGuidance::new(
            "Management assessment has changed or has not been recorded",
            "management.record_required and contract_fingerprint",
            "Consume required context; record known observations once without inventing missing facts",
            "Scope, dependencies, execution outcome or waiting state changes",
        )
    } else {
        ActionGuidance::new(
            "Required context is complete and no higher-priority blocker is present",
            "Current work, claims and management requirements",
            "Consume the rendered context and follow this work's next_action using the existing session/claim rules",
            "Scope change, wait, uncertain result, completed compaction or handoff",
        )
    };
    if serde_json::to_vec(&guidance)?.len() > ACTION_GUIDANCE_MAX_BYTES {
        return Err(Error::InvalidInput(
            "action guidance exceeds its fixed byte budget".into(),
        ));
    }
    // Incomplete/error results retain every existing field, including their diagnostics.
    let complete = value["context"]["completeness"]["complete"] == true;
    value["ok"] = json!(complete);
    value = crate::summarize_work_response(value);
    let mut omitted = value["response_view"]["omitted_fields"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if complete {
        // This provenance is also in the identity. Omit it only on exact equality;
        // never merge different source versions or alter the rendered packet/hash.
        let sources = &value["context"]["work_context"]["identity"]["source_versions"];
        if sources.is_array() && sources == &value["context"]["completeness"]["source_versions"] {
            value["context"]["completeness"]
                .as_object_mut()
                .unwrap()
                .remove("source_versions");
            omitted.push(json!("context.completeness.source_versions"));
        }
        for (parent, field, path) in [
            ("management", "scope", "management.scope"),
            ("management", "next_action", "management.next_action"),
            ("management", "work", "management.work"),
            ("management", "work_id", "management.work_id"),
            ("management", "work_revision", "management.work_revision"),
            ("management", "source_ref", "management.source_ref"),
            ("management", "branch_id", "management.branch_id"),
        ] {
            if value[parent]
                .as_object_mut()
                .and_then(|p| p.remove(field))
                .is_some()
            {
                omitted.push(json!(path));
            }
        }
        if value["management"]["decision"]
            .as_object_mut()
            .and_then(|p| p.remove("optional_maintenance"))
            .is_some()
        {
            omitted.push(json!("management.decision.optional_maintenance"));
        }
        if value
            .as_object_mut()
            .unwrap()
            .remove("next_action")
            .is_some()
        {
            omitted.push(json!("next_action"));
        }
    }
    value["guidance"] = json!(guidance);
    if let Some(c) = compaction.filter(|c| !c["observation_event_id"].is_null()) {
        value["continuity"]["compaction"] = json!({"state":c["state"],"observation_event_id":c["observation_event_id"],"post_compaction_percent":c["post_compaction_percent"]});
    }
    value["response_view"] = json!({"version":1,"view":"action","omitted_fields":omitted,"guidance_max_bytes":ACTION_GUIDANCE_MAX_BYTES});
    Ok(value)
}
