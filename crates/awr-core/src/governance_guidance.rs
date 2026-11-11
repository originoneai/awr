//! One bounded instruction for governed work. Admission, containment, gaps, reassessment, budgets
//! and findings each report their own diagnostics; an agent should not have to read them all to
//! know what to do next. This picks the earliest material gap, the one that has to be dealt with
//! before anything after it can be, and says when it applies, on what basis, what to do and what
//! makes it worth checking again. Everything else stays where it is and is loaded on demand.
//!
//! The wording distinguishes a state that could not be established from a denial and from a
//! decision that went stale, because each calls for something else: refreshing, answering the
//! reason, or deciding again on what the work now is.
use crate::ActionGuidance;

pub fn baseline_review_guidance() -> &'static str {
    "Inspect the baseline logs. If this is a behavior failure, record justified_alternative with a discrimination.note explaining it, then obtain independent admission approval: awr work admit --input <file>"
}

pub fn baseline_rerun_guidance(
    work: &str,
    example: &str,
    session: &str,
    revision: u64,
    reason: Option<&str>,
) -> String {
    let command = crate::cli_next_action(&[
        "awr",
        "work",
        "check",
        work,
        "--example",
        example,
        "--phase",
        "baseline",
        "--session",
        session,
        "--expected-revision",
        &revision.to_string(),
    ]);
    let recognized = match reason {
        Some("timeout") => "the check process exceeded its timeout",
        Some("spawn_failed") => "the check command could not be started (spawn_failed)",
        Some(detail) => detail,
        None => "the recorded check could not run; inspect its captured logs",
    };
    format!("Recognized {recognized}. After repair, rerun: {command}")
}

/// A source-ledger starter; authors still supply their contract and resolve the reported gaps.
pub const WORK_DEFINITION_GUIDANCE: &str = r#"Declare in one source edit; replace sample values and fill the reported gaps.
```yaml
definition:
  version: 1
  purpose: implementation
  examples:
    - id: result
      criterion: Copy an acceptance criterion
      kind: positive
      given: A valid input
      when: The operation runs
      then: The expected result
      check: {run: [cargo, test], files: [tests/result.rs]}
```
Then use one work admit call; no stage document is needed. More: awr work help governance."#;

/// On-demand field shapes for definition gaps, kept out of per-work guidance and receipts.
pub const WORK_DEFINITION_GAP_GUIDANCE: &str = r#"Gap codes | Path: YAML | Hint
intent_missing,inputs_unversioned,intent_ref_not_found,intent_ref_stale | definition.intent: [{role: user_decision,decision: <decision>,source_fingerprint: 'sha256:<sha256>'}] | Accepted decision.
contribution_missing,contribution_goal_not_found,contribution_outcome_unknown | definition.contributes: [{goal: <goal>,outcome: <outcome>,claim: <why>,by: <by>}] | Exact success criterion.
granularity_missing | definition.granularity: {judgment: independent_result,reason: <why>,by: <by>} | One result.
responsibility_missing | definition.responsibility: {name: <name>,kind: delivery} | Named owner.
scope_missing | paths: [<path>] | Work scope.
deliverable_missing | definition.deliverable: <what> | Output.
evidence_method_missing | definition.evidence_method: <how> | Proof.
failure_recovery_missing | definition.failure_recovery: <when> | Failure/recovery.
acceptance_missing | acceptance: [<criterion>] | Observable.
examples_missing,example_criterion_unknown | definition.examples: [{criterion: <criterion>,kind: positive,given: <input>,when: <action>,then: <what>}] | Repeat as negative per criterion.
example_id_missing,example_id_duplicate | definition.examples[0].id: <id> | Unique ID per check.
discrimination_missing | definition.discrimination: {method: fail_before_pass_after} | Method.
discrimination_note_missing | definition.discrimination.note: <reason> | For justified_alternative.
parent_not_found,parent_self,parent_cross_workstream,parent_cycle,step_without_parent,side_line_without_parent | definition.parent: <work> | Known; same stream; acyclic.
inherits_not_found | definition.inherits: <work> | Current definition.
tier_conflicts_with_specification | definition.tier: later | Match specification.
side_line_return_missing | definition.side_line.return_condition: <when> | Rejoin.
side_line_stop_missing | definition.side_line.stop_condition: <when> | Stop.
large_work_budget_missing | definition.budget: {rework_cycles: 3} | Large-work budget.
outline_trigger_missing | definition.outline.trigger: <when> | Elaborate when.
outline_not_elaborated | definition.outline: null | Elaborate fully.
Assumptions are not adopted facts; use user_decision or sourced_fact.
source_fingerprint: sha256:<digest> via shasum -a 256 <decision file>."#;

/// The facts the choice reads. They come from the readiness diagnostics and the findings block.
pub struct GovernanceView<'a> {
    /// `(code, detail)` of every readiness diagnostic of the work.
    pub diagnostics: &'a [(String, String)],
    /// Keys of the blocking findings that still hold the completion of the work.
    pub open_blocking_findings: &'a [String],
}

/// The codes in the order they must be dealt with: what makes the state unreadable first, then
/// what the work owes before it can be admitted, then who has to decide, then the gaps and the
/// facts that came up while the work was done.
const ORDER: &[&str] = &[
    "admission_policy_invalid",
    "admission_unknown",
    "reassessment_unknown",
    "baseline_unrunnable",
    "baseline_unclassified",
    "admission_obligations_unmet",
    "admission_missing",
    "admission_pending",
    "admission_denied",
    "admission_stale",
    "admission_suspended",
    "containment_unchecked",
    "containment_missing",
    "containment_pending",
    "containment_denied",
    "containment_stale",
    "containment_not_authorized",
    "containment_suspended",
    "reassessment_required",
    "reassessment_stale",
    "reassessment_budget_required",
    "reassessment_budget_stale",
    "reassessment_budget_escalated",
    "reassessment_not_authorized",
];

fn clip(text: &str, limit: usize) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let flat = flat.trim();
    if flat.chars().count() > limit {
        let cut: String = flat.chars().take(limit).collect();
        format!("{cut}...")
    } else {
        flat.to_string()
    }
}

/// The guidance for the earliest material gap, or nothing when governance asks for nothing.
pub fn governance_guidance(view: &GovernanceView<'_>) -> Option<ActionGuidance> {
    for code in ORDER {
        let Some((_, detail)) = view.diagnostics.iter().find(|(c, _)| c == code) else {
            continue;
        };
        let definition_missing =
            *code == "admission_obligations_unmet" && detail.contains("definition_not_declared");
        // Reserve room for the YAML starter even when the diagnostic uses multibyte characters.
        let display_detail = clip(detail, if definition_missing { 64 } else { 200 });
        let basis = format!("{code}: {display_detail}");
        let (when, next_action, recheck): (&str, &str, &str) = match *code {
            "admission_policy_invalid" => (
                "The admission policy of this source cannot be used",
                "Fix the admission_policy option in the source mapping. The work is held, not treated as ungoverned, until the policy can be read",
                "The source mapping changes and the policy reads",
            ),
            "admission_unknown" | "reassessment_unknown" => (
                "The governance state of this work could not be established",
                "Refresh the sources and prepare again. An unknown state is neither an approval nor a denial, so nothing is claimed or run on it",
                "The sources are refreshed and the state reads",
            ),
            "admission_obligations_unmet" if definition_missing => (
                "The work has no definition yet, and governed work is admitted on one",
                WORK_DEFINITION_GUIDANCE,
                "The definition report shows no gaps",
            ),
            "baseline_unrunnable" | "admission_obligations_unmet"
                if detail.contains("baseline_unrunnable") || *code == "baseline_unrunnable" =>
            {
                (
                    "The baseline check could not run",
                    "Read the recognized loading or startup failure in admission.baselines[].detail, restore the check's ability to run, then use its baseline command",
                    "A runnable baseline records a behavior failure",
                )
            }
            "baseline_unclassified" | "admission_obligations_unmet"
                if detail.contains("baseline_unclassified") || *code == "baseline_unclassified" =>
            {
                (
                    "The baseline failure has no recognized behavior signature",
                    baseline_review_guidance(),
                    "An independent reviewer approves the recorded alternative method",
                )
            }
            "admission_obligations_unmet" => (
                "The work definition leaves obligations unmet before admission",
                "Fill the missing obligations listed in the detail in one source edit, then admit the work with one work admit call; no stage document or step-level task is needed",
                "The definition report shows no gaps",
            ),
            "admission_missing" => (
                "Governed work has no admission decision",
                "Take the contract_fingerprint from work prepare and record one work admit decision with your judgment; for clear small work one call by a declaring agent is enough",
                "The decision is approved and the scope has not moved",
            ),
            "admission_pending" => (
                "An admission proposal waits for a decision",
                "Have an agent with the authority answer the latest proposal; a proposal admits nothing",
                "A decision answers the latest proposal",
            ),
            "admission_denied" => (
                "The latest decision denied admission",
                "Address the reason in the definition or the scope, then propose again with a new request key; the denial stays on record",
                "A later approval replaces the denial",
            ),
            "admission_stale" => (
                "An approved admission no longer matches the work",
                "Decide again on the current contract. A supervisor replanning within the scope that was granted needs no new permission from the user; widening the scope or the intent does",
                "The new decision binds the current contract",
            ),
            "admission_suspended" | "containment_suspended" => (
                "An open gap puts a judgment of this work in question",
                "Close the gap the way its kind requires (sources for a lookup, the experiment run, an authorized choice, the observed change) or withdraw it; decide again only if the work itself changed",
                "The gap is resolved or withdrawn",
            ),
            "containment_unchecked" => (
                "A child of this parent leaves the parent's paths",
                "Move the child back inside the paths of the parent, or widen the parent's paths, which is a new decision on the parent's contract",
                "Every child stays within the parent's paths",
            ),
            "containment_missing" => (
                "The children of the parent have no containment decision",
                "Record one work contain decision for the parent with the change_set_fingerprint from work show; splitting stays an ordinary edit and opens no task",
                "The decision is approved and no child was added or removed",
            ),
            "containment_pending" => (
                "A containment proposal waits for a decision",
                "Have an authorized agent answer the latest containment proposal",
                "A decision answers the proposal",
            ),
            "containment_denied" => (
                "The latest decision rejected this set of children",
                "Change the set of children as the reason asks, then propose the new set; the rejection stays on record",
                "A later approval covers the current set",
            ),
            "containment_stale" => (
                "The approved set of children no longer matches the children",
                "Decide again on the current set; the detail names each child added or removed. A supervisor replanning within the parent's paths needs no new permission from the user",
                "The new decision binds the current set",
            ),
            "containment_not_authorized" => (
                "The containment was approved by an agent the policy does not authorize",
                "Have one of the policy's supervisors decide the set",
                "A supervisor approves the current set",
            ),
            "reassessment_required" => (
                "The records of this work show a method failing again or an outcome nobody knows",
                "Read the facts in the reassessment block of work show, then record one work reassess decision (continue, wait, reuse, alter_method, split or replan); query an unknown outcome before any retry",
                "A further failure, a new unknown outcome or the trigger clearing",
            ),
            "reassessment_stale" => (
                "The reassessment no longer matches the records of this work",
                "Read what changed in the detail and decide again on the current facts",
                "A further failure or the trigger clearing",
            ),
            "reassessment_budget_required" => (
                "The rework or effort budget of this work is spent",
                "Answer the budget with one work reassess decision: deliver with known issues, reduce the scope, replan or escalate; continuing is not an answer",
                "A further return of the work, or a larger budget",
            ),
            "reassessment_budget_stale" => (
                "The decision about the budget no longer matches it",
                "Decide again on the budget as it now stands",
                "A further return of the work, or a larger budget",
            ),
            "reassessment_budget_escalated" => (
                "The spent budget was escalated and waits for someone else",
                "Leave the decision to the one it was escalated to; you cannot answer your own escalation",
                "Someone else decides",
            ),
            "reassessment_not_authorized" => (
                "The budget escalation was answered by an agent the policy does not authorize",
                "Have one of the policy's supervisors answer the escalation",
                "A supervisor decides",
            ),
            _ => continue,
        };
        return Some(ActionGuidance::new(when, &basis, next_action, recheck));
    }
    if !view.open_blocking_findings.is_empty() {
        return Some(ActionGuidance::new(
            "Blocking findings hold the completion of this work",
            &format!(
                "findings.open_blocking: {}",
                clip(&view.open_blocking_findings.join(", "), 200)
            ),
            "Fix what each finding shows and resolve it with how it was dealt with; deferred records need no action before completion",
            "Every blocking finding is resolved",
        ));
    }
    None
}
