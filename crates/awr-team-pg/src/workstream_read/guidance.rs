//! One bounded, deterministic next-step hint. Advice never grants execution.
use awr_core::Id;
use serde::Serialize;
use serde_json::{Value, json};

/// The complete enriched guidance packet must fit as one value. It is never
/// truncated because a partial command could invite callers to guess bindings.
/// Session.checkpoint packets share this same serialized-byte bound.
pub(crate) const EXECUTION_PREPARE_GUIDANCE_MAX_BYTES: usize = 4096;

#[derive(Clone, Debug)]
pub(crate) struct NextExecutionPrepareInputs {
    pub workstream_id: Id,
    pub work_id: String,
    pub coordinator_epoch: String,
    pub expected_project_revision: String,
    pub expected_authority_version: String,
    pub expected_ownership_version: String,
    pub expected_contract_hash: String,
    pub session_id: String,
    pub expected_session_version: String,
    pub claim_id: String,
    pub expected_fence: String,
    pub expected_lease_version: String,
    pub expected_work_version: String,
}

#[derive(Clone, Debug)]
pub(crate) struct NextSessionCheckpointInputs {
    pub workstream_id: Id,
    pub work_id: String,
    pub coordinator_epoch: String,
    pub expected_project_revision: String,
    pub expected_authority_version: String,
    pub expected_ownership_version: String,
    pub expected_contract_hash: String,
    pub session_id: String,
    pub expected_session_version: String,
    pub require_client_info: bool,
}

#[derive(Clone, Debug)]
pub(crate) enum NextCommandInputs {
    ExecutionPrepare(NextExecutionPrepareInputs),
    SessionCheckpoint(NextSessionCheckpointInputs),
}

#[derive(Serialize)]
struct CommandEnvelope<'a, Args: Serialize> {
    protocol_version: u32,
    op: &'static str,
    workstream_id: Id,
    work_id: &'a str,
    coordinator_epoch: &'a str,
    expected_project_revision: &'a str,
    expected_authority_version: &'a str,
    expected_ownership_version: &'a str,
    expected_contract_hash: &'a str,
    args: Args,
}

#[derive(Serialize)]
struct ExecutionPrepareArgsBase<'a> {
    session_id: &'a str,
    expected_session_version: &'a str,
    claim_id: &'a str,
    expected_fence: &'a str,
    expected_lease_version: &'a str,
    expected_work_version: &'a str,
}

#[derive(Serialize)]
struct SessionCheckpointArgsBase<'a> {
    session_id: &'a str,
    expected_session_version: &'a str,
    context_hash: &'a str,
}

fn identity(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}

fn version(value: &str, allow_zero: bool) -> bool {
    !value.is_empty()
        && (value == "0" || !value.starts_with('0'))
        && value.bytes().all(|b| b.is_ascii_digit())
        && value
            .parse::<i64>()
            .is_ok_and(|parsed| parsed >= i64::from(!allow_zero))
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn envelope_ready(
    work_id: &str,
    coordinator_epoch: &str,
    expected_project_revision: &str,
    expected_authority_version: &str,
    expected_ownership_version: &str,
    expected_contract_hash: &str,
    session_id: &str,
    expected_session_version: &str,
) -> bool {
    identity(work_id)
        && identity(coordinator_epoch)
        && version(expected_project_revision, true)
        && version(expected_authority_version, false)
        && version(expected_ownership_version, false)
        && digest(expected_contract_hash)
        && identity(session_id)
        && version(expected_session_version, false)
}

fn execution_prepare_command_base(inputs: &NextExecutionPrepareInputs) -> Option<Value> {
    if !envelope_ready(
        &inputs.work_id,
        &inputs.coordinator_epoch,
        &inputs.expected_project_revision,
        &inputs.expected_authority_version,
        &inputs.expected_ownership_version,
        &inputs.expected_contract_hash,
        &inputs.session_id,
        &inputs.expected_session_version,
    ) || !identity(&inputs.claim_id)
        || !version(&inputs.expected_fence, false)
        || !version(&inputs.expected_lease_version, false)
        || !version(&inputs.expected_work_version, false)
    {
        return None;
    }
    serde_json::to_value(CommandEnvelope {
        protocol_version: 1,
        op: "execution.prepare",
        workstream_id: inputs.workstream_id,
        work_id: inputs.work_id.as_str(),
        coordinator_epoch: &inputs.coordinator_epoch,
        expected_project_revision: &inputs.expected_project_revision,
        expected_authority_version: &inputs.expected_authority_version,
        expected_ownership_version: &inputs.expected_ownership_version,
        expected_contract_hash: &inputs.expected_contract_hash,
        args: ExecutionPrepareArgsBase {
            session_id: &inputs.session_id,
            expected_session_version: &inputs.expected_session_version,
            claim_id: &inputs.claim_id,
            expected_fence: &inputs.expected_fence,
            expected_lease_version: &inputs.expected_lease_version,
            expected_work_version: &inputs.expected_work_version,
        },
    })
    .ok()
}

fn session_checkpoint_command_base(
    inputs: &NextSessionCheckpointInputs,
    context_hash: &str,
) -> Option<Value> {
    if !envelope_ready(
        &inputs.work_id,
        &inputs.coordinator_epoch,
        &inputs.expected_project_revision,
        &inputs.expected_authority_version,
        &inputs.expected_ownership_version,
        &inputs.expected_contract_hash,
        &inputs.session_id,
        &inputs.expected_session_version,
    ) || !digest(context_hash)
    {
        return None;
    }
    serde_json::to_value(CommandEnvelope {
        protocol_version: 1,
        op: "session.checkpoint",
        workstream_id: inputs.workstream_id,
        work_id: inputs.work_id.as_str(),
        coordinator_epoch: &inputs.coordinator_epoch,
        expected_project_revision: &inputs.expected_project_revision,
        expected_authority_version: &inputs.expected_authority_version,
        expected_ownership_version: &inputs.expected_ownership_version,
        expected_contract_hash: &inputs.expected_contract_hash,
        args: SessionCheckpointArgsBase {
            session_id: &inputs.session_id,
            expected_session_version: &inputs.expected_session_version,
            context_hash,
        },
    })
    .ok()
}

fn attach_command_base(
    mut hint: Value,
    expected_op: &str,
    selected: bool,
    base: Option<Value>,
    caller_slots: Value,
    note_prefix: &str,
) -> Value {
    if !selected || hint["action"]["op"] != expected_op {
        return hint;
    }
    let fallback = hint.clone();
    let Some(selected_note) = hint["action"]["note"].as_str().map(str::to_owned) else {
        return hint;
    };
    if let Some(base) = base {
        hint["action"]["command_base"] = base;
        hint["action"]["caller_slots"] = caller_slots;
        hint["action"]["note"] = json!(format!("{note_prefix} {selected_note}"));
    }
    if hint["action"]["command_base"].is_null()
        || serde_json::to_vec(&hint).map_or(true, |bytes| {
            bytes.len() > EXECUTION_PREPARE_GUIDANCE_MAX_BYTES
        })
    {
        fallback
    } else {
        hint
    }
}

pub(crate) fn prepare_execution_hint() -> Value {
    json!({"code":"prepare_execution",
        "when":"your current live claim has no execution",
        "because":["the active own session and current executor match task ownership"],
        "action":{"op":"execution.prepare",
            "note":"Prepare your execution with current work/session/claim versions, a measured input_digest and declared_scope from the contract. Preparation does not admit effects; execution.start must succeed before editing or running work."},
        "recheck_on":"execution preparation, claim, contract, scope or authority changes"})
}

/// Attach only a complete, current server-known base to the selected action.
/// The three caller-known fields remain absent for the caller to supply.
pub(crate) fn with_execution_prepare_command_base(
    hint: Value,
    inputs: NextExecutionPrepareInputs,
) -> Value {
    let selected = hint["code"] == "prepare_execution";
    attach_command_base(
        hint,
        "execution.prepare",
        selected,
        execution_prepare_command_base(&inputs),
        json!(["request_id", "args.input_digest", "args.declared_scope"]),
        "Use command_base unchanged. Fill exactly request_id, args.input_digest and args.declared_scope; measure input_digest from consumed inputs and choose your intended declared_scope subset from work.prepare.data.visible_contract.scope_paths under the same expected_contract_hash.",
    )
}

/// Attach a schema-complete checkpoint envelope. Caller-measured next_action,
/// open_loops, request_id and, when listed, client_info stay absent.
pub(crate) fn with_session_checkpoint_command_base(
    hint: Value,
    inputs: NextSessionCheckpointInputs,
    context_hash: &str,
) -> Value {
    let mut slots = vec!["request_id", "args.next_action", "args.open_loops"];
    if inputs.require_client_info {
        slots.push("args.client_info");
    }
    attach_command_base(
        hint,
        "session.checkpoint",
        true,
        session_checkpoint_command_base(&inputs, context_hash),
        json!(slots),
        "Use command_base unchanged. Fill exactly the listed caller_slots from observed facts; do not guess progress, usage, model, or unfinished work.",
    )
}

pub(crate) fn with_command_base(
    hint: Value,
    inputs: NextCommandInputs,
    context_hash: &str,
) -> Value {
    match inputs {
        NextCommandInputs::ExecutionPrepare(inputs) => {
            with_execution_prepare_command_base(hint, inputs)
        }
        NextCommandInputs::SessionCheckpoint(inputs) => {
            with_session_checkpoint_command_base(hint, inputs, context_hash)
        }
    }
}

/// Advice belongs to the selected session. A different participant's historical
/// execution is display evidence, not admission for the current session.
pub(super) fn session_execution(data: &Value) -> &Value {
    data.get("session_execution").unwrap_or(&data["execution"])
}

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
    if role_hint
        .as_ref()
        .is_some_and(|hint| hint["code"] == "integration_rejected")
        && data["collaboration"]["integration"]["target_changed"] == true
        && data["responsibility"]["relation"] == "owned_by_me"
        && data["responsibility"]["coordination_allowed_advisory"] == true
    {
        return rejected_integration(data, owns_session, fallback);
    }
    // Employee intake needs its actual session and responsibility chain. The
    // shared condition's preparation query remains discovery's entry point.
    match role_hint {
        Some(hint) if hint["code"] != "intake" => hint,
        _ => fallback,
    }
}

/// Revision uses the existing session, claim and execution commands. An old
/// approval does not reopen a settled execution or admit changes to its content.
fn rejected_integration(data: &Value, owns_session: bool, fallback: Value) -> Value {
    let integration = &data["collaboration"]["integration"];
    let execution = session_execution(data);
    if !owns_session
        || data["session"]["state"] != "active"
        || data["claim"]["lease_live"] != true
        || data["responsibility"]["current_executor_matches_client"] != true
        || matches!(
            fallback["code"].as_str(),
            Some("wait_for_change" | "declare_client")
        )
        || !matches!(
            execution["state"].as_str(),
            Some("succeeded" | "failed" | "cancelled")
        )
        || execution["effects_settled"] != true
        || !integration["reviewed_execution_id"].is_string()
    {
        return fallback;
    }
    let revised = execution["state"] == "succeeded"
        && execution["execution_id"] != integration["reviewed_execution_id"];
    json!({"code":if revised {"submission"}else{"prepare_execution"},
        "when":if revised {"your revision execution is settled"}else{"the selected integration was rejected and your prior execution is settled"},
        "because":["the rejection still matches the selected candidate"],
        "action":{"op":if revised {"delivery.submission.describe"}else{"execution.prepare"},
            "note":if revised {
                "Publish the revised result through the authorized channel; select a new candidate bound to the actual target, submit fresh evidence and request independent review. Do not reuse the rejected candidate's approval."
            } else {
                "Prepare a new execution with current versions, measured input_digest and contract scope, then execution.start before edits. Update against the current repository target and retest; preserve the rejected attempt and its approval as history."
            }},
        "recheck_on":"execution, target, candidate, blocker, ownership or permission changes"})
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
    let execution = session_execution(data);
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
        return prepare_execution_hint();
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
                "Batch progress with next_action/open_loops. Stop and report workspace execution before repository push/publication, external review, merge or deployment; use separate delivery operations. Follow execution.terminal_reporting with measured environment digest and truthful workspace_settlement, even with a live V2 lease. Prior or unknown external effects from this run require recovery; never declare no_external_effects=true for them. Omission preserves recovery; tests do not prove settlement."
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

    fn prepare_inputs(identity: &str) -> NextExecutionPrepareInputs {
        NextExecutionPrepareInputs {
            workstream_id: Id::from(1),
            work_id: identity.to_string(),
            coordinator_epoch: identity.to_string(),
            expected_project_revision: i64::MAX.to_string(),
            expected_authority_version: i64::MAX.to_string(),
            expected_ownership_version: i64::MAX.to_string(),
            expected_contract_hash: "a".repeat(64),
            session_id: identity.to_string(),
            expected_session_version: i64::MAX.to_string(),
            claim_id: identity.to_string(),
            expected_fence: i64::MAX.to_string(),
            expected_lease_version: i64::MAX.to_string(),
            expected_work_version: i64::MAX.to_string(),
        }
    }

    fn checkpoint_inputs(identity: &str, require_client_info: bool) -> NextSessionCheckpointInputs {
        NextSessionCheckpointInputs {
            workstream_id: Id::from(1),
            work_id: identity.to_string(),
            coordinator_epoch: identity.to_string(),
            expected_project_revision: i64::MAX.to_string(),
            expected_authority_version: i64::MAX.to_string(),
            expected_ownership_version: i64::MAX.to_string(),
            expected_contract_hash: "a".repeat(64),
            session_id: identity.to_string(),
            expected_session_version: i64::MAX.to_string(),
            require_client_info,
        }
    }

    fn declare_client_hint() -> Value {
        json!({"code":"declare_client",
            "when":"client capabilities have not been declared",
            "because":["client_info is absent"],
            "action":{"op":"session.checkpoint",
                "note":"At the next checkpoint include known client_info and supported/unsupported/unknown observations; never guess model or usage."},
            "recheck_on":"client, model or capability changes"})
    }

    fn report_checkpoint_hint() -> Value {
        json!({"code":"report_at_boundary",
            "when":"phase, tests, blocker, user wait or delivery changes",
            "because":["session is active and no higher-priority condition was found"],
            "action":{"op":"session.checkpoint",
                "note":"Batch progress with next_action/open_loops and available host usage; terminal outcomes alone use execution.report. Continue only under existing admission."},
            "recheck_on":"material work change or lease expiry"})
    }

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
    fn integration_revision_preserves_recovery_wait_and_lease_priority() {
        let mut data = owned();
        data["collaboration"] =
            json!({"integration":{"reviewed_execution_id":"old","target_changed":true}});
        data["execution"] =
            json!({"execution_id":"old","state":"succeeded","effects_settled":true});
        let hint = json!({"code":"integration_rejected"});
        assert_eq!(
            with_collaboration(&data, true, true, Some(hint.clone()))["code"],
            "prepare_execution"
        );
        for (path, value, code) in [
            ("/runtime/recovery_blocked", json!(true), "inspect_recovery"),
            ("/execution/state", json!("unknown"), "reconcile_execution"),
            ("/claim/expires_at_unix_ms", json!(1000), "renew_claim"),
        ] {
            let mut changed = data.clone();
            let (parent, key) = path.rsplit_once('/').unwrap();
            changed.pointer_mut(parent).unwrap()[key] = value;
            assert_eq!(
                with_collaboration(&changed, true, true, Some(hint.clone()))["code"],
                code
            );
        }
        assert_eq!(
            with_collaboration(&data, false, true, Some(hint.clone()))["code"],
            "restore_context"
        );
        assert_eq!(
            with_collaboration(&data, true, false, Some(hint.clone()))["code"],
            "other_client_session"
        );
        data["waiting_user"] = json!(true);
        assert_eq!(
            with_collaboration(&data, true, true, Some(hint.clone()))["code"],
            "wait_for_change"
        );
        data["waiting_user"] = json!(false);
        data["progress"] = json!({"contract_matches_current":true,"stale":false,"phase":"blocked"});
        assert_eq!(
            with_collaboration(&data, true, true, Some(hint.clone()))["code"],
            "wait_for_change"
        );
        data["progress"] = Value::Null;
        data["execution"]["execution_id"] = json!("new");
        let submission = with_collaboration(&data, true, true, Some(hint));
        assert_eq!(submission["action"]["op"], "delivery.submission.describe");
        assert!(submission.to_string().len() < 900);
        for code in ["recovery", "integration_unknown"] {
            assert_eq!(
                with_collaboration(&data, true, true, Some(json!({"code":code})))["code"],
                code
            );
        }
    }

    #[test]
    fn historical_execution_does_not_admit_a_new_session_or_reuse_its_approval() {
        let mut data = owned();
        data["execution"] =
            json!({"execution_id":"prior","state":"succeeded","effects_settled":true});
        data["session_execution"] = Value::Null;
        assert_eq!(select(&data, true, true)["code"], "prepare_execution");
        data["collaboration"] =
            json!({"integration":{"reviewed_execution_id":"prior","target_changed":true}});
        let hint = json!({"code":"integration_rejected"});
        assert_eq!(
            with_collaboration(&data, true, true, Some(hint))["code"],
            "prepare_execution"
        );
        data["session_execution"] = json!({"state":"unknown"});
        assert_eq!(select(&data, true, true)["code"], "reconcile_execution");
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
    fn execution_prepare_packet_is_complete_and_bounded_by_serialized_bytes() {
        let all_quotes = "\"".repeat(128);
        let multibyte = "é".repeat(64);
        assert_eq!(all_quotes.len(), 128);
        assert_eq!(multibyte.len(), 128);
        let mut tested_max = 0;
        for identity in [all_quotes, multibyte] {
            let hint = with_execution_prepare_command_base(
                prepare_execution_hint(),
                prepare_inputs(&identity),
            );
            let bytes = serde_json::to_vec(&hint).unwrap();
            tested_max = tested_max.max(bytes.len());
            assert!(bytes.len() <= EXECUTION_PREPARE_GUIDANCE_MAX_BYTES);
            let action = &hint["action"];
            assert_eq!(
                action["caller_slots"],
                json!(["request_id", "args.input_digest", "args.declared_scope"])
            );
            let base = &action["command_base"];
            assert_eq!(base["op"], "execution.prepare");
            assert!(base.get("request_id").is_none());
            assert!(base["args"].get("input_digest").is_none());
            assert!(base["args"].get("declared_scope").is_none());
            assert_eq!(base["work_id"], identity);
        }
        eprintln!(
            "largest tested execution-prepare guidance packet: {} bytes",
            tested_max
        );
    }

    #[test]
    fn execution_prepare_packet_preserves_the_selected_action_note() {
        let selected_note = "Keep this selected action's existing update and retest instructions.";
        let mut hint = prepare_execution_hint();
        hint["action"]["note"] = json!(selected_note);
        let enriched = with_execution_prepare_command_base(hint, prepare_inputs("id"));
        let note = enriched["action"]["note"].as_str().unwrap();
        assert!(note.starts_with("Use command_base unchanged."));
        assert!(note.ends_with(selected_note));
        assert_eq!(note.matches(selected_note).count(), 1);
        assert!(enriched["action"]["command_base"].is_object());
    }

    #[test]
    fn execution_prepare_packet_overflow_omits_the_entire_base() {
        let mut fallback = prepare_execution_hint();
        fallback["when"] = json!("x".repeat(EXECUTION_PREPARE_GUIDANCE_MAX_BYTES));
        let enriched = with_execution_prepare_command_base(fallback.clone(), prepare_inputs("id"));
        assert_eq!(enriched, fallback);
        assert_eq!(enriched["action"]["op"], "execution.prepare");
        assert!(enriched["action"].get("command_base").is_none());
        assert!(enriched["action"].get("caller_slots").is_none());
        assert!(
            !enriched["action"]["note"]
                .as_str()
                .unwrap()
                .contains("command_base")
        );
    }

    fn assert_checkpoint_packet(
        hint: &Value,
        identity: &str,
        context_hash: &str,
        require_client_info: bool,
    ) {
        let bytes = serde_json::to_vec(hint).unwrap();
        assert!(bytes.len() <= EXECUTION_PREPARE_GUIDANCE_MAX_BYTES);
        let action = &hint["action"];
        let mut slots = vec![
            json!("request_id"),
            json!("args.next_action"),
            json!("args.open_loops"),
        ];
        if require_client_info {
            slots.push(json!("args.client_info"));
        }
        assert_eq!(action["caller_slots"], json!(slots));
        let base = &action["command_base"];
        assert_eq!(base["protocol_version"], 1);
        assert_eq!(base["op"], "session.checkpoint");
        assert_eq!(base["work_id"], identity);
        assert_eq!(base["args"]["session_id"], identity);
        assert_eq!(base["args"]["context_hash"], context_hash);
        assert!(base.get("request_id").is_none());
        assert!(base["args"].get("next_action").is_none());
        assert!(base["args"].get("open_loops").is_none());
        assert!(base["args"].get("client_info").is_none());
        assert!(base["args"].get("progress").is_none());
        assert!(base["args"].get("usage").is_none());
        assert!(base["args"].get("claim_id").is_none());
        assert!(base["args"].get("expected_fence").is_none());
        assert!(base["args"].get("expected_lease_version").is_none());
        assert!(base["args"].get("expected_work_version").is_none());
        assert!(base["args"].get("input_digest").is_none());
        assert!(base["args"].get("declared_scope").is_none());
    }

    #[test]
    fn session_checkpoint_packet_is_complete_and_bounded_by_serialized_bytes() {
        let context_hash = "b".repeat(64);
        let all_quotes = "\"".repeat(128);
        let multibyte = "é".repeat(64);
        assert_eq!(all_quotes.len(), 128);
        assert_eq!(multibyte.len(), 128);
        let mut tested_max = 0;
        for require_client_info in [true, false] {
            for identity in [all_quotes.clone(), multibyte.clone()] {
                let hint = if require_client_info {
                    declare_client_hint()
                } else {
                    report_checkpoint_hint()
                };
                let enriched = with_session_checkpoint_command_base(
                    hint,
                    checkpoint_inputs(&identity, require_client_info),
                    &context_hash,
                );
                let bytes = serde_json::to_vec(&enriched).unwrap();
                tested_max = tested_max.max(bytes.len());
                assert_checkpoint_packet(&enriched, &identity, &context_hash, require_client_info);
            }
        }
        eprintln!(
            "largest tested session-checkpoint guidance packet: {} bytes",
            tested_max
        );
    }

    #[test]
    fn session_checkpoint_packet_preserves_the_selected_action_note() {
        let selected_note = "Keep this selected action's existing client declaration instructions.";
        let mut hint = declare_client_hint();
        hint["action"]["note"] = json!(selected_note);
        let enriched = with_session_checkpoint_command_base(
            hint,
            checkpoint_inputs("id", true),
            &"b".repeat(64),
        );
        let note = enriched["action"]["note"].as_str().unwrap();
        assert!(note.starts_with("Use command_base unchanged."));
        assert!(note.ends_with(selected_note));
        assert_eq!(note.matches(selected_note).count(), 1);
        assert!(enriched["action"]["command_base"].is_object());
    }

    #[test]
    fn session_checkpoint_packet_overflow_omits_the_entire_base() {
        let mut fallback = declare_client_hint();
        fallback["when"] = json!("x".repeat(EXECUTION_PREPARE_GUIDANCE_MAX_BYTES));
        let enriched = with_session_checkpoint_command_base(
            fallback.clone(),
            checkpoint_inputs("id", true),
            &"b".repeat(64),
        );
        assert_eq!(enriched, fallback);
        assert_eq!(enriched["action"]["op"], "session.checkpoint");
        assert!(enriched["action"].get("command_base").is_none());
        assert!(enriched["action"].get("caller_slots").is_none());
        assert!(
            !enriched["action"]["note"]
                .as_str()
                .unwrap()
                .contains("command_base")
        );
    }

    #[test]
    fn session_checkpoint_omits_base_for_unrelated_ops_and_invalid_context_hash() {
        let inputs = checkpoint_inputs("id", true);
        let hash = "b".repeat(64);
        let execution =
            with_session_checkpoint_command_base(prepare_execution_hint(), inputs.clone(), &hash);
        assert_eq!(execution["action"]["op"], "execution.prepare");
        assert!(execution["action"].get("command_base").is_none());
        let routed = with_command_base(
            declare_client_hint(),
            NextCommandInputs::SessionCheckpoint(inputs.clone()),
            &hash,
        );
        assert!(routed["action"]["command_base"].is_object());
        let uppercase = "B".repeat(64);
        let prefixed = format!("sha256:{}", "b".repeat(64));
        for bad in ["", prefixed.as_str(), uppercase.as_str()] {
            let omitted =
                with_session_checkpoint_command_base(declare_client_hint(), inputs.clone(), bad);
            assert_eq!(omitted["action"]["op"], "session.checkpoint");
            assert!(omitted["action"].get("command_base").is_none());
        }
        let execution_routed = with_command_base(
            prepare_execution_hint(),
            NextCommandInputs::ExecutionPrepare(prepare_inputs("id")),
            &hash,
        );
        assert_eq!(execution_routed["action"]["op"], "execution.prepare");
        assert!(execution_routed["action"]["command_base"].is_object());
        assert_eq!(
            execution_routed["action"]["caller_slots"],
            json!(["request_id", "args.input_digest", "args.declared_scope"])
        );
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
        assert!(note.contains("Stop and report workspace execution before repository push/publication, external review, merge or deployment"));
        assert!(note.contains("use separate delivery operations"));
        assert!(note.contains("Prior or unknown external effects from this run require recovery"));
        assert!(note.contains("never declare no_external_effects=true"));
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
