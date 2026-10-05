//! One bounded, deterministic next-step hint. Advice never grants execution.
use serde_json::{Value, json};

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
    } else {
        (
            "report_at_boundary",
            "phase, tests, blocker, user wait or delivery changes",
            "session is active and no higher-priority condition was found",
            "session.checkpoint",
            "Batch progress with next_action/open_loops and available host usage; terminal outcomes alone use execution.report. Continue only under existing admission.",
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
}
