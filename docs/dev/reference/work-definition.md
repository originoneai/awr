# Work definitions

A work definition records, next to the work item in a YAML ledger, what the work is for and what
must be settled before it is built: the adopted intent, the outcome it advances, who is
responsible, and how its tests will show they detect a wrong implementation. It reuses the
existing goals, success criteria, acceptance, specification references, decisions and dependencies;
it adds no goal, task or requirement system and no second status. It is optional. A work item
without a definition is projected, shown and prepared exactly as before.

The check is deterministic. It verifies that references resolve, that pinned versions are current
and that the obligations of the declared purpose are present. It does not judge whether a
contribution claim, a granularity judgment or an example is adequate; those stay attributed
judgments of the author and reviewer. Declaring a definition grants no execution permission and
changes no completion rule.

## Declare one

```yaml
work_items:
  - id: SAVE-1
    kind: feature                     # unchanged; the purpose below is separate
    acceptance: ["Saving failure keeps the user's input"]
    paths: [crates/storage/src]
    definition:
      version: 1
      purpose: implementation         # implementation | research | document | decision
      tier: required                  # required | later
      parent: SAVE-0                  # optional; same workstream, no cycle
      inherits: SAVE-0                # optional; a current parent definition may satisfy obligations
      intent:
        - role: user_decision         # user_decision | sourced_fact | assumption | suggestion
          decision: scope-decision    # a registered, accepted decision
          source_fingerprint: "sha256:…"   # the exact bytes adopted
          note: The first release is local only
        - role: assumption
          note: Users save rarely
      open_gaps: [Offline behavior is undecided]
      adoption: {basis: Scope confirmed in the planning thread, by: person-1}
      contributes:
        - goal: G-1
          outcome: "Users keep their input when saving fails"   # an exact success criterion of G-1
          claim: The failure path keeps the draft
          by: agent-a
      granularity: {judgment: independent_result, reason: One result a reviewer can judge, by: agent-a}
      responsibility: {name: storage-module, kind: delivery}    # design | delivery
      deliverable: Local save with a failure fallback
      evidence_method: Targeted tests compared before and after the change
      failure_recovery: A failed save keeps the previous data
      examples:
        - {criterion: "Saving failure keeps the user's input", kind: positive,
           given: the storage is read-only, when: the user saves, then: the input is still shown}
        - {criterion: "Saving failure keeps the user's input", kind: negative,
           given: the storage is read-only, when: the user saves, then: success is never reported}
      discrimination: {method: fail_before_pass_after}   # or diff_mutation, justified_alternative
      side_line:                      # optional: work spawned from another line (needs a parent)
        return_condition: The failure modes are listed and reviewed
        stop_condition: Two days of effort, or a decision to drop the screen
      outline:                        # optional: distant work; never admitted until elaborated
        constraints: [No new storage layer]
        trigger: When the API contract is accepted
```

Only YAML ledgers support the field; a Markdown ledger rejects it with rule `ledger.definition`.
The shape is strict: unknown fields are refused, every list holds at most 64 entries, text is
nonblank and at most 4096 bytes, and the whole record stays within 64 KiB. Text is screened like
other public content, so it must not contain secret values. User decisions and sourced facts need
a decision key and a `sha256:` fingerprint; assumptions and suggestions are kept as such and are
never treated as adopted facts. The first version is authored by editing the source; `work
create` and `batch change` do not write it.

## What is checked

`awr work show WORK` and `awr work prepare WORK` (and `awr_work_get`, `awr_work_prepare`) add a
`definition` block only when one is declared. It lists each obligation as `satisfied`, `missing`,
`unknown`, `stale` or `not_applicable`, a list of `gaps` that keeps the reason, and informational
`notes`. `state` is `complete` or `gaps`.

| Purpose | Obligations |
| --- | --- |
| `implementation` | all of the rows below |
| `research`, `document` | purpose, intent, contribution, granularity, deliverable |
| `decision` | purpose, intent, contribution, deliverable |

| Gap code | Meaning |
| --- | --- |
| `intent_missing` | no user decision or sourced fact pins the adopted intent |
| `intent_ref_not_found`, `intent_ref_stale` | the decision is not registered, accepted and visible, or its bytes changed after adoption |
| `contribution_missing`, `contribution_goal_not_found`, `contribution_outcome_unknown` | no contribution, an unknown goal, or an outcome that is not one of the goal's exact success criteria |
| `granularity_missing`, `step_without_parent` | no attributed judgment; a step of a parent that names none |
| `parent_not_found`, `parent_self`, `parent_cross_workstream`, `parent_cycle` | the parent relation is inconsistent; cross-workstream relations use dependencies |
| `responsibility_missing` | no named design or delivery responsibility; a link by identifier alone never stands in for it |
| `inputs_unversioned`, `scope_missing` | neither specification references nor pinned decisions fix the input versions; no allowed paths |
| `deliverable_missing`, `evidence_method_missing`, `failure_recovery_missing` | the corresponding statement is absent |
| `acceptance_missing` | the work states no acceptance criterion, so nothing observable can be asserted |
| `examples_missing`, `example_criterion_unknown` | a required criterion lacks a positive and a negative example, or an example names text that is not an acceptance criterion |
| `discrimination_missing` | no method says how the tests will show they detect a wrong implementation |
| `tier_conflicts_with_specification` | the work is marked required while every specification item it references is declared future |
| `inherits_not_found` | the inherited work declares no definition |
| `side_line_without_parent`, `side_line_return_missing`, `side_line_stop_missing` | a side line does not name the line it returns to, when its result returns, or when it stops |
| `outline_trigger_missing`, `outline_not_elaborated` | an outline has no elaboration trigger; an outline is not near-term executable, so [admission](admission.md) refuses it until it is elaborated |

Two obligations take their answer from existing sources instead of the definition: the observable
assertions are the work's acceptance criteria (the examples refine them), and the prerequisites are
its dependency links. A work without dependencies reports `prerequisites` as `not_applicable`; that
is never a gap, and unfinished prerequisites stay the business of the readiness rules.

The report reads project-wide facts, so shared workstream grants never receive it: for a project
that uses workstreams, the shared service refuses `awr_work_get` and `awr_work_prepare` and points
to `awr_workstream`, which carries no definition.

An outline keeps distant work visible without pretending it is ready: it owes only its purpose and the
outcome it contributes to, plus a trigger that says when to elaborate it, and its constraints are kept
as stated. Elaborating it means replacing the outline with a full definition, which then owes
everything its purpose owes. A side line owes a contribution like any work, and additionally a parent to
return to and the two conditions; none of them is a new task type or state.

The note `parent_outcome_unproven` appears on a parent whose children are all completed while the
parent is not: finished children never prove the parent's own outcome, which still needs its own
acceptance evidence.

A criterion is `later` when the work is marked `tier: later` or when every specification
reference bound to exactly that criterion points at a `future` item; later criteria owe no
examples or discrimination method. An empty or inherited obligation is satisfied only while the
inherited definition is itself current.

## Relation to other checks

A source that opts in to [plan admission](admission.md) reads this report for the entry obligations
of the declared purpose: a definition with gaps blocks admission, and work that declares none is
judged as an implementation.

A declared definition is part of the management contract fingerprint (see
[management intensity](management.md)): editing it requires a new assessment, while progress,
next-action and evidence changes do not. Work without a definition keeps its existing
fingerprint. Like [specification coverage](specification-coverage.md), the check reads indexed
sources and never writes; a clean report does not establish that the work is correct, only that
its declared obligations are present and current.
