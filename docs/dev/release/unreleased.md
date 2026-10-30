# Unreleased

Changes shipped in 0.6.0 are listed in [0.6.0](0.6.0.md).

## Added

- Explicit requirement items and YAML task `spec_refs`, with bounded, read-only
  `awr spec check` and MCP `awr_spec_check` views of one evaluator. Checks identify
  changed or superseded item bindings, missing task associations and contradictions
  with decision `affected_keys`; applicability never infers requirement coverage.
  References reuse the existing task-contract fingerprint and evidence rules.
  Run `awr source reindex` after upgrading to refresh the changed projections.
  Markdown task bindings and cross-document replacement traversal are deferred.
  See [specification references and coverage](../reference/specification-coverage.md).
- Optional work definitions in YAML ledgers: a work item can record its purpose, adopted intent,
  the goal outcome it advances, responsibility, examples and how its tests will show they detect a
  wrong implementation. `awr work show` and `awr work prepare` (and the MCP work tools) report each
  obligation of the declared purpose and every gap by code, only for work that declares one. The
  check is deterministic and read-only, grants no permission and changes no completion rule. See
  [work definitions](../reference/work-definition.md).
- Opt-in governed plan admission per ledger source. Under `governed_enforce`, claiming work, a
  handoff's claim transfer, a resume's claim inheritance and registering or starting an execution
  need a recorded, approved decision that still describes the current contract, definition,
  reviewed examples and policy; `awr work admit` and the MCP tool `awr_work_admit` record
  proposals and decisions as events (no schema migration). Projects without a policy are
  unaffected, `legacy_observe` reports without blocking, and recovery paths and completion policy
  are unchanged. See [plan admission](../reference/admission.md).
- Controlled decomposition for governed sources: the children of a parent are admitted only while a
  containment decision on exactly this set is current (`awr work contain`, MCP `awr_work_contain`).
  The decision binds the parent's contract, each child's definition, paths and acceptance, and the
  dependencies between them; added or removed children, widened paths and changed dependencies make
  an approval stale by name, and a child outside the parent's paths cannot be approved. Work
  definitions gain optional `side_line` and `outline` declarations. See
  [controlled decomposition](../reference/containment.md).
- Bounded reassessment for governed sources: the program classifies what a work's own records show
  (failed methods, outcomes nobody knows, unanswered waits, declared blockers) and, only for the work
  they belong to, asks for a current decision (`awr work reassess`, MCP `awr_work_reassess`) after
  three failures of one method in a row or while an outcome is unknown. A decision binds exactly
  those facts, goes stale when they change and opens no task; failures that alternate with
  successes, waits and blockers are shown and hold nothing, and recovery paths are never held. See
  [bounded reassessment](../reference/reassessment.md).
- Gaps for governed sources (`awr work gap`, MCP `awr_work_gap`): something missing or changed in a
  work's intent, acceptance, design, plan or definition is recorded against that layer and closed
  the way its kind requires, by a fact lookup with its sources, a feasibility experiment that
  really ran (repairing the plan never closes it, a failed run counts), a choice by an authorized
  agent or an observed environment change. An open gap puts in question only the admission or
  containment judgments that read its layer, on that work; it keeps every failure on record, opens
  no task and holds no unrelated work. See [gaps](../reference/gaps.md).
