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
        let detail = clip(detail, 200);
        let basis = format!("{code}: {detail}");
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
            "admission_obligations_unmet" if detail.contains("definition_not_declared") => (
                "The work has no definition yet, and governed work is admitted on one",
                "Declare the work definition in one source edit (purpose, adopted intent, outcome contribution, responsibility, deliverable, examples and a discrimination method, as the purpose needs), then admit it with one work admit call; no stage document, confirmation round or step-level task is needed",
                "The definition report shows no gaps",
            ),
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
