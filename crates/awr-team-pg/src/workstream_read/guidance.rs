//! One bounded, deterministic next-step hint. Advice never grants execution.
use serde_json::{Value, json};

/// Protective execution advice retains priority over coordination and delivery.
pub(super) fn with_collaboration(
    data: &Value,
    context_complete: bool,
    owns_session: bool,
    role_hint: Option<Value>,
) -> Value {
    let fallback = select(data, context_complete, owns_session);
    if matches!(
        fallback["code"].as_str(),
        Some(
            "restore_context"
                | "confirm_controlled_execution"
                | "reconcile_execution"
                | "inspect_recovery"
                | "refresh_execution"
                | "inspect_expired_claim"
        )
    ) {
        return fallback;
    }
    if fallback["code"] == "renew_claim"
        && !role_hint.as_ref().is_some_and(|hint| {
            matches!(
                hint["code"].as_str(),
                Some("recovery" | "integration_unknown")
            )
        })
    {
        return fallback;
    }
    // Employee intake needs its actual session and responsibility chain. The
    // shared condition's preparation query remains discovery's entry point.
    match role_hint {
        Some(hint) if hint["code"] != "intake" => hint,
        _ => fallback,
    }
}

/// Preserve legacy action.op while keeping one bounded factual basis. A read
/// grant must not suggest an unauthorized session or lease mutation.
pub(super) fn authorized(
    auth: &super::ReaderAuthority,
    stream: awr_core::Id,
    work: &str,
    mut hint: Value,
) -> Value {
    use awr_team::Action;
    if hint["code"] == "assignment"
        && crate::delegation_auth::assignment_read_authority(auth, stream, work).is_ok()
    {
        hint["action"] = json!({"op":"task.assignees",
            "query":{"protocol_version":1,"op":"task.assignees","work_id":work,"workstream_id":stream},
            "note":"Find an eligible member, refresh work.prepare, then use awr_team_command/task.assign with assignee_person_id and expected_responsibility_version from responsibility.version. Existing-work assignment needs no planning draft. Leave unassigned work available for self-claim."});
    }
    let action = match hint["action"]["op"].as_str() {
        Some("session.start" | "session.checkpoint") => Some(Action::SessionMaintainOwn),
        Some(
            "claim.acquire" | "claim.renew" | "task.accept_assignment" | "task.claim_available",
        ) => Some(Action::ClaimManageOwn),
        Some("execution.prepare" | "execution.start") => Some(Action::ExecutionRequestAndReportOwn),
        _ => None,
    };
    if action.is_some_and(|a| {
        !super::inbox::permits(
            auth,
            a,
            hint["action"]["op"].as_str().unwrap(),
            stream,
            work,
        )
    }) {
        hint = json!({"code":"inspect_only","when":"the suggested mutation is outside current authority",
            "because":"no current covering action and write grant","action":{"op":"work.next",
            "note":"Inspect your visible work; obtain the required scoped grant before session, responsibility or execution mutations."},
            "recheck_on":"member, delegation, source or workstream permissions change"});
    }
    if let Some(basis) = hint["because"].as_str() {
        hint["because"] = json!([basis]);
    }
    hint
}

pub(super) fn select(data: &Value, context_complete: bool, owns_session: bool) -> Value {
    let runtime = &data["runtime"];
    let execution = &data["execution"];
    let claim = &data["claim"];
    let progress = &data["progress"];
    let execution_resolved = matches!(
        execution["state"].as_str(),
        Some("succeeded" | "failed" | "cancelled")
    );
    let (code, when, because, op, action, recheck) = if !context_complete {
        (
            "restore_context",
            "required context is incomplete",
            "missing authorized specs or dependencies",
            "work.prepare",
            "Restore missing context before effects; use source.content or a larger context budget as needed.",
            "specification or dependency changes",
        )
    } else if execution["state"] == "unknown"
        && execution["controlled_confirmation_available"] == true
    {
        (
            "confirm_controlled_execution",
            "your admitted controlled run has an attributed report barrier",
            "current grant, run and exact reserved paths still match",
            "execution.attest",
            "Inspect the latest receipt; attest only your controlled, stopped run with reviewed_receipt_id and facts.executor_stopped=true. Remaining effects keep recovery blocked.",
            "receipt, grant, epoch, scope or recovery changes",
        )
    } else if execution["state"] == "unknown" {
        (
            "reconcile_execution",
            "execution effects remain unresolved",
            "current execution or work requires recovery",
            "execution.inspect",
            "Inspect the latest receipt and obtain authorized reconciliation before further effects.",
            "new receipt or reconciliation",
        )
    } else if runtime["recovery_blocked"] == true {
        (
            "inspect_recovery",
            "work requires recovery",
            "a work or operator recovery barrier remains",
            "work.recovery",
            "Inspect work recovery and involve the authorized operator for any remaining restore barrier before effects.",
            "work or operator recovery changes",
        )
    } else if runtime["state"] == "completed" || !runtime["selected_completion_id"].is_null() {
        (
            "inspect_completion",
            "a completion is recorded",
            "current work runtime has a completion",
            "completion.inspect",
            "Inspect the completion and select further work through work.next.",
            "completion invalidation or new work",
        )
    } else if runtime["state"] == "cancelled" || runtime["state"] == "archived" {
        (
            "work_inactive",
            "work is inactive",
            "current runtime is cancelled or archived",
            "work.next",
            "Select eligible work; a historical checkpoint is not a resume instruction.",
            "work reopened",
        )
    } else if !execution_resolved
        && (execution["contract_matches_current"] == false
            || execution["epoch_matches_current"] == false)
    {
        (
            "refresh_execution",
            "execution binding changed",
            "contract or coordinator epoch no longer matches",
            "execution.inspect",
            "Inspect and reconcile the earlier execution; consume current work.prepare before proceeding.",
            "contract, epoch or recovery changes",
        )
    } else if execution["state"] == "running" && execution["lease_live"] != true {
        (
            "inspect_expired_claim",
            "a running execution has no live lease",
            "execution lease observation",
            "execution.inspect",
            "Stop effects. Expired leases cannot be renewed or resume an old run. Inspect the latest execution receipt and settle pending effects with an authorized operator before work.prepare, a fresh claim and a new execution.start.",
            "reconciliation or new claim",
        )
    } else if data["pr_deliveries"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|p| p["state"] == "active" && p["contract_matches_current"] == true)
    }) {
        (
            "inspect_delivery",
            "a current PR is already registered",
            "versioned delivery exists for the current contract",
            "delivery.inspect",
            "Inspect delivery and review. Use delivery.register_pr for a changed head; delivery.observe_pr only updates the bound head. Do not infer acceptance from CI.",
            "PR head, evidence or review changes",
        )
    } else if data["session"].is_null() {
        (
            "start_session",
            "no current session is recorded",
            "work observation has no current-ownership session",
            "session.start",
            "Start your own session with known client_info after consuming work.prepare; leave unavailable observations unknown.",
            "session created or authority changes",
        )
    } else if !owns_session {
        (
            "other_client_session",
            "the observed session belongs to another client",
            "authenticated identity does not own this session",
            "work.next",
            "Find your own session or eligible work; do not update another client's checkpoint.",
            "claim, handoff or ownership changes",
        )
    } else if data["session"]["state"] != "active" {
        (
            "inspect_recovery",
            "the session is closed",
            "session state is not active",
            "work.recovery",
            "Inspect the saved work before starting a new session; closed sessions cannot report progress.",
            "successor session or recovery changes",
        )
    } else if claim["state"] == "active"
        && claim["lease_live"] == true
        && claim["expires_at_unix_ms"]
            .as_i64()
            .zip(data["observed_at_unix_ms"].as_i64())
            .is_some_and(|(expiry, now)| expiry.saturating_sub(now) <= 60_000)
    {
        (
            "renew_claim",
            "your live lease expires within one minute",
            "current server lease expiry",
            "claim.renew",
            "Renew with current lease versions before further effects; renewal is not an execution result.",
            "renewal, revocation or expiry",
        )
    } else if !claim.is_null()
        && claim["lease_live"] != true
        && (claim["state"] == "active" || claim["state"] == "expired")
        && !(execution_resolved
            && execution["effects_settled"] == true
            && execution["recovery_blocked"] != true
            && execution["previous_epoch_review_required"] != true
            && claim["epoch_matches_current"] == true)
    {
        (
            "inspect_expired_claim",
            "the execution lease is no longer live",
            "lease expired or coordinator epoch changed",
            "claim.inspect",
            "Stop effects and inspect execution/recovery. Expired leases cannot be renewed; settle pending effects before work.prepare and a fresh claim. Do not resume an old execution.",
            "reconciliation or new claim",
        )
    } else if data["waiting_user"] == true {
        (
            "wait_for_change",
            "a user wait is open",
            "current work has an unresolved wait item",
            "work.recovery",
            "Wait for the required reply and refresh context; checkpoint any material progress without starting more effects.",
            "user reply or wait closure",
        )
    } else if progress["contract_matches_current"] == true
        && progress["stale"] == false
        && (progress["phase"] == "waiting_user" || progress["phase"] == "blocked")
    {
        (
            "wait_for_change",
            "a blocker or user wait was reported",
            "latest current structured progress",
            "session.checkpoint",
            "Retain the blocker and next action; report again when a reply or material fact changes, without guessing progress.",
            "user reply or blocker resolution",
        )
    } else if data["client"].is_null() {
        (
            "declare_client",
            "client capabilities have not been declared",
            "client_info is absent",
            "session.checkpoint",
            "At the next checkpoint include known client_info and supported/unsupported/unknown observations; never guess model or usage.",
            "client, model or capability changes",
        )
    } else if data["responsibility"]["relation"] == "owned_by_me"
        && data["responsibility"]["current_executor_matches_client"] == true
        && claim["state"] == "active"
        && claim["lease_live"] == true
        && execution.is_null()
    {
        (
            "prepare_execution",
            "your current live claim has no execution",
            "the active own session and current executor match task ownership",
            "execution.prepare",
            "Prepare your execution with current work/session/claim versions, a measured input_digest and declared_scope from the contract. Preparation does not admit effects; execution.start must succeed before editing or running work.",
            "execution preparation, claim, contract, scope or authority changes",
        )
    } else if data["responsibility"]["relation"] == "owned_by_me"
        && data["responsibility"]["current_executor_matches_client"] == true
        && claim["state"] == "active"
        && claim["lease_live"] == true
        && execution["state"] == "prepared"
        && execution["owned_by_client"] == true
        && execution["lease_live"] == true
        && execution["contract_matches_current"] == true
        && execution["epoch_matches_current"] == true
        && execution["cancel_requested"] == false
    {
        (
            "start_execution",
            "your bound execution is prepared and its lease is live",
            "the prepared execution matches current ownership, contract and epoch",
            "execution.start",
            "Start the exact prepared execution using its receipt's ID and input digest with current work/session/claim/execution versions. Choose the contract's supported execution mode; perform effects only after successful admission.",
            "admission, cancellation, claim, contract, epoch or authority changes",
        )
    } else {
        (
            "report_at_boundary",
            "phase, tests, blocker, user wait or delivery changes",
            "session is active and no higher-priority condition was found",
            "session.checkpoint",
            if execution["terminal_reporting"].is_object() {
                "Batch progress with next_action/open_loops. At actual termination follow execution.terminal_reporting: include workspace_settlement with a measured environment digest and truthful stop/effect assertions, even with a live V2 lease. Omission preserves recovery; tests do not prove settlement."
            } else {
                "Batch progress with next_action/open_loops and available host usage; terminal outcomes alone use execution.report. Continue only under existing admission."
            },
            "material work change or lease expiry",
        )
    };
    json!({"code":code,"when":when,"because":because,"action":{"op":op,"note":action},"recheck_on":recheck})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active() -> Value {
        json!({"runtime":{"state":"in_progress"},"session":{"state":"active"},"client":{"product":"Agent"},
            "claim":{"state":"active","lease_live":true,"expires_at_unix_ms":200000},"observed_at_unix_ms":1})
    }

    fn owned() -> Value {
        let mut data = active();
        data["definition_state"] = json!("enabled");
        data["responsibility"] = json!({"relation":"owned_by_me",
            "current_executor_matches_client":true,"coordination_allowed_advisory":true});
        data
    }

    #[test]
    fn current_owned_intake_connects_preparation_to_exact_admission() {
        let mut data = owned();
        let prepare = select(&data, true, true);
        assert_eq!(prepare["code"], "prepare_execution");
        assert_eq!(prepare["action"]["op"], "execution.prepare");
        assert!(
            prepare["action"]["note"]
                .as_str()
                .unwrap()
                .contains("input_digest")
        );
        data["execution"] = json!({"state":"prepared","owned_by_client":true,
            "lease_live":true,"contract_matches_current":true,"epoch_matches_current":true,
            "cancel_requested":false});
        let start = select(&data, true, true);
        assert_eq!(start["code"], "start_execution");
        assert_eq!(start["action"]["op"], "execution.start");
        assert!(
            start["action"]["note"]
                .as_str()
                .unwrap()
                .contains("exact prepared execution")
        );
        for hint in [prepare, start] {
            assert!(hint.to_string().len() < 900);
            for key in ["when", "because", "action", "recheck_on"] {
                assert!(!hint[key].is_null());
            }
        }
    }

    #[test]
    fn admission_advice_requires_live_current_ownership_and_preparation() {
        for relation in [
            "pool",
            "assigned_to_me",
            "owned_by_other",
            "handoff_required",
        ] {
            let mut data = owned();
            data["responsibility"]["relation"] = json!(relation);
            assert_ne!(select(&data, true, true)["code"], "prepare_execution");
        }
        let mut data = owned();
        data["responsibility"]["current_executor_matches_client"] = json!(false);
        assert_ne!(select(&data, true, true)["code"], "prepare_execution");
        data = owned();
        assert_eq!(select(&data, true, false)["code"], "other_client_session");
        data["execution"] = json!({"state":"prepared","owned_by_client":true,
            "lease_live":true,"contract_matches_current":true,"epoch_matches_current":true,
            "cancel_requested":false});
        for field in [
            "owned_by_client",
            "lease_live",
            "contract_matches_current",
            "epoch_matches_current",
        ] {
            let mut changed = data.clone();
            changed["execution"][field] = json!(false);
            assert_ne!(select(&changed, true, true)["code"], "start_execution");
        }
        data["execution"]["cancel_requested"] = json!(true);
        assert_ne!(select(&data, true, true)["code"], "start_execution");
    }

    #[test]
    fn entry_advice_preserves_protective_and_collaboration_priorities() {
        for execution in [
            Value::Null,
            json!({"state":"prepared","owned_by_client":true,
            "lease_live":true,"contract_matches_current":true,"epoch_matches_current":true,
            "cancel_requested":false}),
        ] {
            let mut data = owned();
            data["execution"] = execution;
            assert_eq!(select(&data, false, true)["code"], "restore_context");
            data["runtime"]["recovery_blocked"] = json!(true);
            assert_eq!(select(&data, true, true)["code"], "inspect_recovery");
            data["runtime"]["recovery_blocked"] = json!(false);
            data["claim"]["expires_at_unix_ms"] = json!(1000);
            assert_eq!(select(&data, true, true)["code"], "renew_claim");
            data["claim"]["lease_live"] = json!(false);
            assert_eq!(select(&data, true, true)["code"], "inspect_expired_claim");
            data = owned();
            data["waiting_user"] = json!(true);
            assert_eq!(select(&data, true, true)["code"], "wait_for_change");
            data["waiting_user"] = json!(false);
            for code in [
                "review",
                "rework",
                "verification",
                "integration_unknown",
                "dependency",
            ] {
                let role = json!({"code":code,"action":{"op":"work.observe"}});
                assert_eq!(
                    with_collaboration(&data, true, true, Some(role))["code"],
                    code
                );
            }
        }
    }

    #[test]
    fn running_and_terminal_runs_are_not_implicitly_prepared_again() {
        let mut data = owned();
        for state in ["running", "succeeded", "failed", "cancelled"] {
            data["execution"] = json!({"state":state,"owned_by_client":true,
                "lease_live":true,"contract_matches_current":true,"epoch_matches_current":true});
            assert_eq!(select(&data, true, true)["code"], "report_at_boundary");
        }
    }

    #[test]
    fn guidance_prioritizes_facts_and_never_repeats_checkpoint_instructions() {
        let mut data = active();
        data["checkpoint"] = json!({"next_action":"Register the PR"});
        data["pr_deliveries"] = json!([{"state":"active","contract_matches_current":true}]);
        assert_eq!(select(&data, true, true)["code"], "inspect_delivery");
        data["execution"] = json!({"state":"unknown"});
        assert_eq!(select(&data, true, true)["code"], "reconcile_execution");
        assert_eq!(select(&data, false, true)["code"], "restore_context");
        data["runtime"]["state"] = json!("completed");
        assert_eq!(select(&data, true, true)["code"], "reconcile_execution");
        data["execution"] = Value::Null;
        assert_eq!(select(&data, true, true)["code"], "inspect_completion");
    }

    #[test]
    fn reporting_is_conditional_bounded_and_scoped_to_the_owning_client() {
        let mut data = active();
        assert_eq!(select(&data, true, true)["code"], "report_at_boundary");
        assert_eq!(select(&data, true, false)["code"], "other_client_session");
        data["claim"]["expires_at_unix_ms"] = json!(1000);
        assert_eq!(select(&data, true, true)["code"], "renew_claim");
        data["claim"]["lease_live"] = json!(false);
        assert_eq!(select(&data, true, true)["code"], "inspect_expired_claim");
        data["claim"] = Value::Null;
        data["progress"] =
            json!({"phase":"waiting_user","contract_matches_current":true,"stale":false});
        assert_eq!(select(&data, true, true)["code"], "wait_for_change");
        data["progress"]["stale"] = json!(true);
        assert_eq!(select(&data, true, true)["code"], "report_at_boundary");
        for facts in [data, Value::Null, active()] {
            let hint = select(&facts, true, true);
            assert!(hint.to_string().len() < 900);
            for key in ["when", "because", "action", "recheck_on"] {
                assert!(!hint[key].is_null());
            }
        }
    }

    #[test]
    fn workspace_boundary_hint_preserves_protective_priority_and_single_action() {
        let mut data = active();
        data["execution"] = json!({"state":"running","lease_live":true,
            "terminal_reporting":{"action":{"op":"execution.report"}}});
        let hint = select(&data, true, true);
        assert_eq!(hint["code"], "report_at_boundary");
        assert_eq!(hint["action"]["op"], "session.checkpoint");
        let note = hint["action"]["note"].as_str().unwrap();
        assert!(note.contains("workspace_settlement"));
        assert!(note.contains("live V2 lease"));
        assert!(note.contains("tests do not prove settlement"));
        assert!(hint.to_string().len() < 900);
        assert_eq!(select(&data, true, false)["code"], "other_client_session");
        assert_eq!(select(&data, false, true)["code"], "restore_context");
        data["execution"]["state"] = json!("unknown");
        assert_eq!(select(&data, true, true)["code"], "reconcile_execution");
        data["execution"]["state"] = json!("running");
        data["execution"]["lease_live"] = json!(false);
        assert_eq!(select(&data, true, true)["code"], "inspect_expired_claim");
    }

    #[test]
    fn elapsed_claim_after_explicit_settlement_keeps_current_work_actions() {
        for outcome in ["succeeded", "failed", "cancelled"] {
            for claim_state in ["active", "expired"] {
                let mut data = active();
                data["claim"] = json!({"state":claim_state,"lease_live":false,
                    "epoch_matches_current":true});
                data["execution"] = json!({"state":outcome,"effects_settled":true,
                    "recovery_blocked":false,"previous_epoch_review_required":false});
                let hint = select(&data, true, true);
                assert_eq!(hint["code"], "report_at_boundary");
                assert!(hint.to_string().len() < 900);
                assert_eq!(select(&data, true, false)["code"], "other_client_session");
                assert_eq!(select(&data, false, true)["code"], "restore_context");
                let review = json!({"code":"review","action":{"op":"review.inspect"}});
                assert_eq!(
                    with_collaboration(&data, true, true, Some(review))["code"],
                    "review"
                );
                data["waiting_user"] = json!(true);
                assert_eq!(select(&data, true, true)["code"], "wait_for_change");
                data["waiting_user"] = json!(false);
                data["execution"]["effects_settled"] = Value::Null;
                assert_eq!(select(&data, true, true)["code"], "inspect_expired_claim");
                data["execution"]["effects_settled"] = json!(true);
                data["claim"]["epoch_matches_current"] = json!(false);
                assert_eq!(select(&data, true, true)["code"], "inspect_expired_claim");
                data["claim"]["epoch_matches_current"] = json!(true);
                data["execution"]["previous_epoch_review_required"] = json!(true);
                assert_eq!(select(&data, true, true)["code"], "inspect_expired_claim");
                data["runtime"]["recovery_blocked"] = json!(true);
                assert_eq!(select(&data, true, true)["code"], "inspect_recovery");
            }
        }
    }

    #[test]
    fn restored_work_without_an_execution_uses_work_recovery() {
        let mut data = active();
        data["runtime"]["recovery_blocked"] = json!(true);
        for execution in [
            Value::Null,
            json!({"state":"succeeded","recovery_blocked":true}),
            json!({"state":"failed","recovery_blocked":true}),
            json!({"state":"cancelled","recovery_blocked":true}),
        ] {
            data["execution"] = execution;
            let hint = select(&data, true, true);
            assert_eq!(hint["code"], "inspect_recovery");
            assert_eq!(hint["action"]["op"], "work.recovery");
        }
    }

    #[test]
    fn resolved_execution_binding_changes_do_not_create_a_recovery_loop() {
        let mut data = active();
        for state in ["succeeded", "failed", "cancelled"] {
            data["execution"] = json!({"state":state,"contract_matches_current":false,"epoch_matches_current":false});
            assert_eq!(select(&data, true, true)["code"], "report_at_boundary");
        }
        data["execution"]["state"] = json!("running");
        assert_eq!(select(&data, true, true)["code"], "refresh_execution");
    }

    #[test]
    fn controlled_confirmation_is_conditional_and_stays_bounded() {
        let mut data = active();
        data["runtime"]["recovery_blocked"] = json!(true);
        data["execution"] = json!({"state":"unknown","controlled_confirmation_available":true});
        let hint = select(&data, true, true);
        assert_eq!(hint["action"]["op"], "execution.attest");
        assert!(hint.to_string().len() < 900);
        assert_eq!(select(&data, false, true)["code"], "restore_context");
        data["execution"]["controlled_confirmation_available"] = json!(false);
        assert_eq!(select(&data, true, true)["code"], "reconcile_execution");
    }
}
