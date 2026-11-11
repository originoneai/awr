# Work definitions

A work definition records a work item's purpose, adopted intent, contribution, responsibility and
acceptance examples in its YAML ledger. Optional example identifiers and check bindings connect
each example to a command and a file list. Definition reports show the declared obligations,
reference currency and whether each example has a check.

## Use it

Check support in each reader, then inspect a work item's definition:

```sh
awr capabilities --require definition.example_checks
awr work show SAVE-1 --json
awr work prepare SAVE-1 --json
```

For a minimal YAML starter available in the installed CLI, run:

```sh
awr work help governance
```

Activation refusals caused by `definition_not_declared` and the text output of
`work show` point to this same help topic. Copy its `definition` block beside the
work item, replace the sample criterion, command and file path, and fill the remaining
reported obligations. The starter declares a definition; admission still requires
the work's complete contract and applicable checks.

Author the definition beside the work item in its source ledger:

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
        - id: draft-remains
          criterion: "Saving failure keeps the user's input"
          kind: positive
          given: the storage is read-only
          when: the user saves
          then: the input is still shown
          check:
            run: [cargo, test, -p, storage, draft_remains]
            files: [crates/storage/tests/save.rs]
        - id: success-withheld
          criterion: "Saving failure keeps the user's input"
          kind: negative
          given: the storage is read-only
          when: the user saves
          then: success is never reported
          check:
            run: [cargo, test, -p, storage, success_withheld]
            files: [crates/storage/tests/save.rs]
            timeout_seconds: 300
      discrimination: {method: fail_before_pass_after}   # or diff_mutation, justified_alternative
      side_line:                      # optional: work spawned from another line (needs a parent)
        return_condition: The failure modes are listed and reviewed
        stop_condition: Two days of effort, or a decision to drop the screen
      outline:                        # optional: distant work; never admitted until elaborated
        constraints: [No new storage layer]
        trigger: When the API contract is accepted
      size: medium                    # optional: small, medium (the default) or large
      budget:                         # optional: replaces the defaults of the size
        rework_cycles: 3
        active_effort_minutes: 240
```

## How it behaves

| Layer | Responsibility |
| --- | --- |
| Definition | Declared obligations and pinned references |
| Admission | Current oracle files and failed baselines |
| Work check | Program-recorded check execution |

The deterministic check verifies reference resolution, pinned versions and the obligations of the
declared purpose. Definitions reuse existing goals, success criteria, acceptance, specification
references, decisions and dependencies. Work without a definition keeps its existing projection,
display and preparation behavior. Legacy examples keep `id` and `check` absent when serialized.

YAML ledgers support definitions. Unknown definition, example and check fields are rejected. Lists hold at most 64 entries; descriptive text is nonblank and at most 4096 bytes; records fit within 64 KiB. Public-content screening covers the complete definition. Adopted user decisions and sourced facts require a decision key and `sha256:` fingerprint.

| Example field | Rule |
| --- | --- |
| `id` | Optional; matches `[a-z0-9][a-z0-9-]{0,31}` and is unique within the definition. Required when a check is declared. |
| `check.run` | An argument array with 1–64 entries, each at most 1024 bytes. It declares the command directly, with the project root as its working directory. |
| `check.files` | 1–32 project-relative file paths. Parent components (`..`), absolute paths and the root `.awr` directory are rejected; both path separators are checked. |
| `check.timeout_seconds` | Optional timeout budget of 1–7200 seconds; the default budget is 600 seconds. |

`awr work show WORK` and `awr work prepare WORK` (and `awr_work_get`, `awr_work_prepare`) add a
`definition` block only when one is declared. It lists each obligation as `satisfied`, `missing`,
`unknown`, `stale` or `not_applicable`, a list of `gaps` that keeps the reason, and informational
`notes`. `state` is `complete` or `gaps`. The `examples` array contains up to 64 summaries in source
order, each with `id` (null when absent), `kind`, `criterion` and `has_check`.

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
| `example_id_missing` | an example declares a check without an identifier |
| `example_id_duplicate` | an identifier repeats within the definition |
| `discrimination_missing` | no method says how the tests will show they detect a wrong implementation |
| `tier_conflicts_with_specification` | the work is marked required while every specification item it references is declared future |
| `inherits_not_found` | the inherited work declares no definition |
| `side_line_without_parent`, `side_line_return_missing`, `side_line_stop_missing` | a side line does not name the line it returns to, when its result returns, or when it stops |
| `large_work_budget_missing` | a work declared `large` has no rework budget of its own: larger work declares it, and until it does the figure of a standard work applies |
| `outline_trigger_missing`, `outline_not_elaborated` | an outline has no elaboration trigger; an outline is not near-term executable, so [admission](admission.md) refuses it until it is elaborated |

Observable assertions come from work acceptance criteria, refined by examples. Prerequisites come from dependency links. An empty dependency set reports `prerequisites=not_applicable`.

The note `example_check_absent` identifies each example for a required criterion that has no check.
It leaves the definition's `state` unchanged and adds no gap by itself.

An outline keeps distant work visible without pretending it is ready: it owes only its purpose and the
outcome it contributes to, plus a trigger that says when to elaborate it, and its constraints are kept
as stated. Elaborating it means replacing the outline with a full definition, which then owes
everything its purpose owes. A side line owes a contribution like any work, and additionally a parent to
return to and the two conditions; none of them is a new task type or state.

`size` and `budget` bound rework cycles and active effort before a decision is required. See [bounded reassessment](reassessment.md#budgets).

The `parent_outcome_unproven` note marks a parent whose children have all completed while the parent remains open.

A criterion is `later` when the work is marked `tier: later` or when every specification
reference bound to exactly that criterion points at a `future` item; later criteria owe no
examples or discrimination method. An empty or inherited obligation is satisfied only while the
inherited definition is itself current.

### Relation to other checks

A source that opts in to [plan admission](admission.md) reads this report for the entry obligations
of the declared purpose: a definition with gaps blocks admission, and work that declares none is
judged as an implementation.

A declared definition is part of the management contract fingerprint (see
[management intensity](management.md)): editing it requires a new assessment, while progress,
next-action and evidence changes do not. Work without a definition keeps its existing
fingerprint. Example identifiers and all check fields are included in the examples fingerprint;
changing a check argument makes an approved [admission](admission.md) stale. Work `summary` also
remains part of the contract, while `next_action` and `priority` remain outside it.
Like [specification coverage](specification-coverage.md), the report uses indexed sources.
Admission also reads the files of required check-bound examples and freezes their current hashes.
The effective `fail_before_pass_after` method requires a failed baseline per such example;
see [check freezing and baselines](admission.md#freeze-checks-and-record-baselines).

## Limits

- Definitions use YAML; Markdown ledgers reject them with `ledger.definition`.
- Assumptions and suggestions retain their declared roles rather than becoming adopted facts.
- Absent prerequisites create no definition gap; readiness rules judge unfinished dependencies.
- Size and budget configure reassessment thresholds rather than adding definition obligations.
- Parent outcomes require the parent’s own acceptance evidence; child completion alone is insufficient.
- Definitions reuse existing goals, tasks, requirements and statuses.
- Authors and reviewers judge the semantic adequacy of contribution claims, granularity and examples.
- Definitions declare obligations; execution admission and completion retain their own rules.
- Definition validation checks declarations. Admission reads `check.files`; `work check` executes `check.run`.
- File existence, content and symlink targets are checked by later operations rather than definition validation.
- Older readers reject `id` and `check`; upgrade every reader before adding them to shared ledgers.
- Author definitions in authoritative YAML; `work create` and `batch change` support other planning fields.
- Shared workstream grants use `awr_workstream`; the project-wide definition report is outside their read scope.
- A complete report establishes current declared obligations. Correct implementation and accepted delivery require their own evidence.

## Code and tests

- Model and report: `crates/awr-core/src/work_definition.rs`.
- Definition rules: `crates/awr-core/tests/work_definition.rs`.
- Source parsing: `crates/awr-source/tests/ledger_definition.rs`.
- Admission bindings: `crates/awr-runtime/tests/admission.rs`.
- CLI reports and capability negotiation: `crates/awr-cli/tests/work_definition_cli.rs`.
